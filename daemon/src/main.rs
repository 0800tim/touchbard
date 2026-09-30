//! touchbard: a themed Touch Bar for Omarchy on Apple Silicon MacBook Pros.
//!
//! Runs as a system service from boot (it must own the Touch Bar's display
//! before Hyprland starts). A per-user agent connects over a Unix socket and
//! supplies the theme, the layout and live state; see ../agent.

mod fallback;
mod hw;
mod proto;
mod render;
mod ui;

use anyhow::{Context as _, Result};
use input::event::keyboard::{KeyState, KeyboardEventTrait};
use input::event::switch::{Switch, SwitchEvent, SwitchState};
use input::event::touch::{TouchEvent, TouchEventPosition, TouchEventSlot};
use input::event::{DeviceEvent, Event, EventTrait, KeyboardEvent};
use input::{Libinput, LibinputInterface};
use input_linux::Key;
use proto::*;
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::{Duration, Instant};
use ui::{Effect, Model};

const SOCKET: &str = "/run/touchbard/touchbard.sock";
const STATE_DIR: &str = "/var/lib/touchbard";

struct Interface;

impl LibinputInterface for Interface {
    fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<OwnedFd, i32> {
        let mode = flags & libc::O_ACCMODE;
        OpenOptions::new()
            .custom_flags(flags)
            .read(mode == libc::O_RDONLY || mode == libc::O_RDWR)
            .write(mode == libc::O_WRONLY || mode == libc::O_RDWR)
            .open(path)
            .map(Into::into)
            .map_err(|e| e.raw_os_error().unwrap_or(libc::EIO))
    }
    fn close_restricted(&mut self, fd: OwnedFd) {
        drop(File::from(fd));
    }
}

struct Client {
    stream: UnixStream,
    /// Peer UID from SO_PEERCRED, taken when the connection was accepted.
    uid: u32,
    buf: Vec<u8>,
    is_agent: bool,
    /// A frame header waiting for its payload: (plugin id, width, height, bytes).
    pending: Option<(String, u32, u32, usize)>,
}

/// Turn a plugin's raw BGRA frame into a surface we can draw.
fn frame_surface(w: u32, h: u32, data: Vec<u8>) -> Option<cairo::ImageSurface> {
    if w == 0 || h == 0 || data.len() != (w * h * 4) as usize {
        return None;
    }
    cairo::ImageSurface::create_for_data(data, cairo::Format::Rgb24, w as i32, h as i32, (w * 4) as i32).ok()
}

fn decode_art(b64: &str) -> Option<cairo::ImageSurface> {
    use base64::Engine;
    if b64.is_empty() {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    cairo::ImageSurface::create_from_png(&mut std::io::Cursor::new(bytes)).ok()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--preview") {
        if let Err(e) = preview(&args[2..]) {
            eprintln!("preview: {e:#}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(e) = run() {
        eprintln!("touchbard: {e:#}");
        std::process::exit(1);
    }
}

fn load_theme() -> Theme {
    fs::read_to_string(format!("{STATE_DIR}/theme.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn drop_privileges() -> Result<()> {
    use nix::unistd::{chown, setgid, setgroups, setuid, Group, User};
    if std::env::var_os("TOUCHBARD_KEEP_ROOT").is_some() || !nix::unistd::getuid().is_root() {
        return Ok(());
    }
    let user = User::from_name("nobody")?.context("no user 'nobody'")?;
    let groups: Vec<_> = ["input", "video"]
        .iter()
        .filter_map(|g| Group::from_name(g).ok().flatten().map(|g| g.gid))
        .collect();
    let _ = fs::create_dir_all(STATE_DIR);
    let _ = chown(STATE_DIR, Some(user.uid), Some(user.gid));
    setgroups(&groups)?;
    setgid(user.gid)?;
    setuid(user.uid)?;
    Ok(())
}

fn listen() -> Result<UnixListener> {
    let path = std::env::var("TOUCHBARD_SOCKET").unwrap_or_else(|_| SOCKET.into());
    if let Some(dir) = Path::new(&path).parent() {
        fs::create_dir_all(dir)?;
    }
    let _ = fs::remove_file(&path);
    let l = UnixListener::bind(&path).with_context(|| format!("bind {path}"))?;
    // Any local user can reach the socket, but only the owner of the active
    // desktop session (or root) gets past `authorized`: see there.
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666))?;
    l.set_nonblocking(true)?;
    Ok(l)
}

fn peer_uid(s: &UnixStream) -> Option<u32> {
    use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};
    getsockopt(s, PeerCredentials).ok().map(|c| c.uid())
}

/// The user who owns the active session on seat0, per logind. Layouts carry
/// commands that the agent runs in that user's session, so nobody else may
/// talk to us. Re-read on every check: it changes when users switch.
fn active_uid() -> Option<u32> {
    let seat = fs::read_to_string("/run/systemd/seats/seat0").ok()?;
    seat.lines().find_map(|l| l.strip_prefix("ACTIVE_UID=")?.trim().parse().ok())
}

fn authorized(uid: u32) -> bool {
    uid == 0 || active_uid() == Some(uid)
}

/// Send to every authorized client, dropping any that aren't (a user who
/// switched away) or whose connection broke.
fn send_all(clients: &mut Vec<Client>, msg: &Outgoing) {
    let mut line = serde_json::to_vec(msg).unwrap_or_default();
    line.push(b'\n');
    clients.retain_mut(|c| {
        if !authorized(c.uid) {
            return false;
        }
        match c.stream.write_all(&line) {
            Ok(()) => true,
            Err(e) => e.kind() == ErrorKind::WouldBlock,
        }
    });
}

fn run() -> Result<()> {
    let mut out = hw::Output::open().context("opening the Touch Bar display")?;
    let mut backlight = hw::Backlight::open()?;
    let keyboard = hw::Keyboard::open().context("creating the virtual keyboard")?;
    let listener = listen()?;

    let mut tb_input = Libinput::new_with_udev(Interface);
    tb_input.udev_assign_seat("seat-touchbar").map_err(|_| anyhow::anyhow!("seat-touchbar"))?;
    let mut main_input = Libinput::new_with_udev(Interface);
    main_input.udev_assign_seat("seat0").map_err(|_| anyhow::anyhow!("seat0"))?;

    drop_privileges().context("dropping privileges")?;

    let (w, h) = out.size();
    let mut model = Model::new(w as f64, h as f64, load_theme(), fallback::layout());
    let mut frame = cairo::ImageSurface::create(cairo::Format::ARgb32, w as i32, h as i32)?;
    let mut clients: Vec<Client> = vec![];
    let mut dirty = true;
    // Frames are capped at ~30 fps: spectrum messages and animation ticks
    // arrive independently and would otherwise each trigger a redraw.
    const FRAME: Duration = Duration::from_millis(33);
    let mut last_frame = Instant::now() - FRAME;
    // What we last told the agent: the plugin surfaces on screen, and whether the bar is lit.
    let mut last_specs: Option<Vec<SurfaceSpec>> = None;
    let mut was_off: Option<bool> = None;
    let mut last_activity = Instant::now();
    let mut lid_closed = false;
    let mut last_minute = chrono::Local::now().format("%H%M").to_string();
    let mut swallowed: HashSet<u32> = HashSet::new();
    let mut digitizer: Option<input::Device> = None;
    let debug = std::env::var_os("TOUCHBARD_DEBUG").is_some();

    eprintln!("touchbard: {}x{} panel ready", out.dev_w, out.dev_h);

    loop {
        let now = Instant::now();
        dirty |= model.tick(now);

        // Minute-resolution clock; battery is cheap to re-read on the same beat.
        let minute = chrono::Local::now().format("%H%M").to_string();
        if minute != last_minute {
            last_minute = minute;
            dirty = true;
        }

        // Backlight: follow the main display, dim when idle, off when idle longer.
        let s = &model.layout.settings;
        // Stay awake for a Touch ID prompt, and while the visualiser plays.
        if model.finger.state != FingerState::Idle || (model.viz.is_some() && model.flag("playing")) {
            last_activity = now;
        }
        let idle = now - last_activity;
        let display = backlight.display_level();
        let full = ((s.max_brightness as f64) * display.sqrt()).round().max(1.0) as u32;
        let target = if lid_closed || display <= 0.0 || idle > Duration::from_secs(s.off_after) {
            0
        } else if idle > Duration::from_secs(s.dim_after) {
            (full / 4).max(1)
        } else {
            full
        };
        backlight.set(target);
        if was_off != Some(backlight.is_off()) {
            was_off = Some(backlight.is_off());
            send_all(&mut clients, &Outgoing::Power { on: !backlight.is_off() });
        }
        let specs = if backlight.is_off() { vec![] } else { model.surface_specs() };
        if last_specs.as_ref() != Some(&specs) {
            send_all(&mut clients, &Outgoing::Surfaces { list: specs.clone() });
            last_specs = Some(specs);
        }

        let frame_wanted = dirty || model.animating(now);
        let frame_due = last_frame + FRAME;
        if frame_wanted && now >= frame_due {
            last_frame = now;
            {
                let c = cairo::Context::new(&frame)?;
                render::draw(&model, &c, now);
            }
            frame.flush();
            if let Err(e) = out.present(&mut frame) {
                eprintln!("present: {e:#}");
            }
            dirty = false;
        }

        // Sleep until something happens or something is due.
        let mut deadline = model.next_deadline(now).unwrap_or(now + Duration::from_secs(5));
        if dirty {
            deadline = deadline.min(frame_due.max(now));
        }
        let secs_left = 60 - chrono::Timelike::second(&chrono::Local::now()) as u64;
        deadline = deadline.min(now + Duration::from_secs(secs_left.max(1)));
        if !backlight.is_off() {
            for t in [s.dim_after, s.off_after] {
                let at = last_activity + Duration::from_secs(t) + Duration::from_millis(50);
                if at > now {
                    deadline = deadline.min(at);
                }
            }
        }
        let timeout = deadline.saturating_duration_since(now).as_millis().min(60_000) as i32;

        let mut fds = vec![
            libc::pollfd { fd: tb_input.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: main_input.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        ];
        fds.extend(clients.iter().map(|c| libc::pollfd {
            fd: c.stream.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }));
        unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, timeout) };

        let mut effects: Vec<Effect> = vec![];

        // ---- input ----
        tb_input.dispatch()?;
        main_input.dispatch()?;
        let events: Vec<Event> = tb_input.by_ref().chain(main_input.by_ref()).collect();
        for event in events {
            match &event {
                Event::Device(DeviceEvent::Added(d)) => {
                    let dev = d.device();
                    if debug {
                        eprintln!("device added: '{}'", dev.name());
                    }
                    // Match on capability, not name: our own virtual keyboard is
                    // called "Touch Bar ..." too, and would otherwise win.
                    if dev.has_capability(input::DeviceCapability::Touch) && dev.name().contains("Touch Bar") {
                        digitizer = Some(dev);
                        // It reappears after resume; put our picture back too.
                        let _ = out.restore();
                        dirty = true;
                    }
                    continue;
                }
                Event::Device(_) => continue,
                Event::Switch(SwitchEvent::Toggle(t)) => {
                    if t.switch() == Some(Switch::Lid) {
                        lid_closed = t.switch_state() == SwitchState::On;
                        effects.extend(model.release_all());
                    }
                    last_activity = now;
                    continue;
                }
                Event::Keyboard(KeyboardEvent::Key(k)) => {
                    last_activity = now;
                    // Enter submits a typed password, which stops the fingerprint
                    // scan without fprintd saying so; take the prompt down with it.
                    // Only a press, and only once the prompt has been up a moment:
                    // the Enter that launched `sudo` is still being released just
                    // as the prompt appears, and must not clear it.
                    let is_enter = k.key() == Key::Enter as u32 || k.key() == Key::KpEnter as u32;
                    if is_enter
                        && k.key_state() == KeyState::Pressed
                        && model.finger.state == FingerState::Scan
                        && now - model.finger.since > Duration::from_millis(1000)
                    {
                        model.set_finger(FingerState::Idle);
                        dirty = true;
                    }
                    if k.key() == Key::Fn as u32 {
                        effects.extend(model.set_fn(k.key_state() == KeyState::Pressed));
                        dirty = true;
                    }
                    continue;
                }
                _ if debug && event.device().name().contains("Touch Bar") && !matches!(event, Event::Touch(_) | Event::Device(_)) => {
                    eprintln!("non-touch event from Touch Bar: {event:?}");
                }
                Event::Touch(te) => {
                    if debug {
                        let pos = match te {
                            TouchEvent::Down(d) => format!("down x={:.0}", d.x_transformed(w)),
                            TouchEvent::Up(_) => "up".into(),
                            _ => String::new(),
                        };
                        if !pos.is_empty() {
                            eprintln!(
                                "touch {pos} from '{}' (digitiser {:?}, backlight off {})",
                                te.device().name(),
                                digitizer.as_ref().map(|d| d.name().to_string()),
                                backlight.is_off()
                            );
                        }
                    }
                    if Some(te.device()) != digitizer {
                        continue;
                    }
                    let was_off = backlight.is_off();
                    last_activity = now;
                    match te {
                        TouchEvent::Down(d) => {
                            if was_off {
                                // First touch on a dark bar only wakes it.
                                swallowed.insert(d.seat_slot());
                            } else {
                                effects.extend(model.touch_down(d.seat_slot(), d.x_transformed(w)));
                            }
                        }
                        TouchEvent::Motion(m) => {
                            if !swallowed.contains(&m.seat_slot()) {
                                effects.extend(model.touch_motion(m.seat_slot(), m.x_transformed(w)));
                            }
                        }
                        TouchEvent::Up(u) => {
                            if !swallowed.remove(&u.seat_slot()) {
                                effects.extend(model.touch_up(u.seat_slot()));
                            }
                        }
                        TouchEvent::Cancel(c) => {
                            swallowed.remove(&c.seat_slot());
                            effects.extend(model.touch_up(c.seat_slot()));
                        }
                        _ => {}
                    }
                    dirty = true;
                }
                _ => {
                    // Pointer and gesture events on the main seat count as activity.
                    if event.device().name() != "" {
                        last_activity = now;
                    }
                }
            }
        }

        // ---- clients ----
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    let uid = peer_uid(&stream);
                    match uid {
                        Some(uid) if authorized(uid) && stream.set_nonblocking(true).is_ok() => {
                            let mut hello = serde_json::to_vec(&Outgoing::Hello).unwrap_or_default();
                            hello.push(b'\n');
                            let mut stream = stream;
                            let _ = stream.write_all(&hello);
                            clients.push(Client { stream, uid, buf: vec![], is_agent: false, pending: None });
                        }
                        // Dropping the stream closes it: not the session owner.
                        _ => eprintln!("touchbard: refused connection from uid {uid:?}"),
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
        let mut agent_gone = false;
        let mut i = 0;
        while i < clients.len() {
            // Checked before reading, so nothing from a non-owner is ever parsed.
            if !authorized(clients[i].uid) {
                agent_gone |= clients[i].is_agent;
                clients.remove(i);
                continue;
            }
            let mut closed = false;
            let mut chunk = [0u8; 16384];
            loop {
                match clients[i].stream.read(&mut chunk) {
                    Ok(0) => {
                        closed = true;
                        break;
                    }
                    Ok(n) => clients[i].buf.extend_from_slice(&chunk[..n]),
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(_) => {
                        closed = true;
                        break;
                    }
                }
            }
            loop {
                // A frame's raw payload follows its header line.
                if let Some((id, w, h, len)) = clients[i].pending.clone() {
                    if clients[i].buf.len() < len {
                        break;
                    }
                    let data: Vec<u8> = clients[i].buf.drain(..len).collect();
                    clients[i].pending = None;
                    if let Some(surf) = frame_surface(w, h, data) {
                        model.surfaces.insert(id, (surf, Instant::now()));
                        dirty = true;
                    }
                    continue;
                }
                let Some(pos) = clients[i].buf.iter().position(|b| *b == b'\n') else { break };
                let line: Vec<u8> = clients[i].buf.drain(..=pos).collect();
                match serde_json::from_slice::<Incoming>(&line) {
                    Ok(msg) => {
                        dirty = true;
                        match msg {
                            Incoming::Theme(t) => {
                                if let Ok(s) = serde_json::to_string(&t) {
                                    let _ = fs::write(format!("{STATE_DIR}/theme.json"), s);
                                }
                                model.theme = t;
                            }
                            Incoming::Layout(l) => {
                                clients[i].is_agent = true;
                                last_specs = None;
                                was_off = None;
                                effects.extend(model.set_layout(l));
                            }
                            Incoming::State { values } => model.update_state(values),
                            Incoming::Fingerprint { s } => model.set_finger(s),
                            Incoming::Layer { name } => effects.extend(model.switch_layer(&name)),
                            Incoming::Bars { v } => model.set_bars(v),
                            Incoming::Art { png } => model.art = decode_art(&png),
                            Incoming::Pixels { id, w, h, len } => clients[i].pending = Some((id, w, h, len)),
                        }
                    }
                    Err(e) => eprintln!("bad message: {e}"),
                }
            }
            if closed || clients[i].buf.len() > 32 << 20 {
                agent_gone |= clients[i].is_agent;
                clients.remove(i);
            } else {
                i += 1;
            }
        }
        if agent_gone && !clients.iter().any(|c| c.is_agent) {
            effects.extend(model.set_layout(fallback::layout()));
            model.set_finger(FingerState::Idle);
            dirty = true;
        }

        // ---- effects ----
        for fx in effects {
            match fx {
                Effect::KeyDown(k) => keyboard.keys(&k, true),
                Effect::KeyUp(k) => keyboard.keys(&k, false),
                Effect::Send(msg) => send_all(&mut clients, &msg),
            }
        }
    }
}

// ---- preview ----------------------------------------------------------------

/// Render one frame to a PNG, for designing without touching the hardware:
/// touchbard --preview out.png [layout.json] [theme.json] [key=value ...]
fn preview(args: &[String]) -> Result<()> {
    let out = args.first().context("usage: --preview out.png [layout.json] [theme.json] [k=v...]")?;
    let mut layout = fallback::layout();
    let mut theme = Theme::default();
    let mut model_opts = vec![];
    for a in &args[1..] {
        if a.ends_with(".json") {
            let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(a)?)?;
            match v.get("t").and_then(|t| t.as_str()) {
                Some("theme") => theme = serde_json::from_value(v)?,
                _ => layout = serde_json::from_value(v)?,
            }
        } else {
            model_opts.push(a.clone());
        }
    }
    let (w, h) = (2008, 60);
    let mut model = Model::new(w as f64, h as f64, theme, layout);
    let mut age = 0.0;
    for o in model_opts {
        let (k, v) = o.split_once('=').unwrap_or((&o, ""));
        match k {
            "layer" => {
                model.switch_layer(v);
            }
            "finger" => model.set_finger(serde_json::from_value(serde_json::Value::from(v))?),
            "age" => age = v.parse()?,
            "viz" => {
                model.open_viz();
                if let Some(z) = &mut model.viz {
                    z.mode = v.to_string();
                }
            }
            "style" => model.bar_style = v.parse()?,
            "text" => model.text_style = v.parse()?,
            "mood" => model.mood = Some(v.to_string()),
            "scrub" => {
                if let Some(z) = &mut model.viz {
                    z.scrub = Some(v.parse()?);
                }
            }
            "state" => {
                let vals: std::collections::HashMap<String, serde_json::Value> =
                    serde_json::from_str(&fs::read_to_string(v)?)?;
                model.update_state(vals);
            }
            "tapweather" => {
                model.weather = Some(Instant::now());
            }
            "art" => model.art = Some(cairo::ImageSurface::create_from_png(&mut File::open(v)?)?),
            "bars" => {
                // A made-up spectrum: bass-heavy with some sparkle.
                let n: usize = v.parse()?;
                let bars = (0..n)
                    .map(|i| {
                        let t = i as f32 / n as f32;
                        ((1.0 - t).powf(1.4) * 0.8 + 0.25 * ((t * 23.0).sin() * 0.5 + 0.5)).min(1.0)
                    })
                    .collect();
                model.set_bars(bars);
            }
            "tap" => {
                let x: f64 = v.parse()?;
                model.touch_down(0, x);
                model.touch_up(0);
            }
            "press" => {
                model.touch_down(1, v.parse()?);
            }
            _ => {
                let val = serde_json::from_str(v).unwrap_or(serde_json::Value::from(v));
                model.update_state([(k.to_string(), val)].into());
            }
        }
    }
    let surf = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h)?;
    if let Ok(n) = std::env::var("TOUCHBARD_BENCH").map(|v| v.parse::<u32>().unwrap_or(300)) {
        // Frame cost for performance work: TOUCHBARD_BENCH=300 touchbard --preview ...
        let t = Instant::now();
        for _ in 0..n {
            let c = cairo::Context::new(&surf)?;
            render::draw(&model, &c, Instant::now());
        }
        let per = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
        eprintln!("{n} frames: {per:.2} ms/frame ({:.1}% of one core at 30 fps)", per * 30.0 / 10.0);
    }
    {
        let c = cairo::Context::new(&surf)?;
        render::draw(&model, &c, Instant::now() + Duration::from_secs_f64(age));
    }
    surf.write_to_png(&mut File::create(out)?)?;
    Ok(())
}
