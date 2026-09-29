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
    buf: Vec<u8>,
    is_agent: bool,
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
    // Anyone may connect; `accept` checks who they are.
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666))?;
    l.set_nonblocking(true)?;
    Ok(l)
}

fn peer_allowed(s: &UnixStream) -> bool {
    use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};
    getsockopt(s, PeerCredentials).is_ok_and(|c| c.uid() == 0 || c.uid() >= 1000)
}

fn send_all(clients: &mut Vec<Client>, msg: &Outgoing) {
    let mut line = serde_json::to_vec(msg).unwrap_or_default();
    line.push(b'\n');
    clients.retain_mut(|c| match c.stream.write_all(&line) {
        Ok(()) => true,
        Err(e) => e.kind() == ErrorKind::WouldBlock,
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
    let mut last_activity = Instant::now();
    let mut lid_closed = false;
    let mut last_minute = chrono::Local::now().format("%H%M").to_string();
    let mut swallowed: HashSet<u32> = HashSet::new();
    let mut digitizer: Option<input::Device> = None;

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
        if model.finger.state != FingerState::Idle {
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

        if dirty || model.animating(now) {
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
                    if dev.name().contains("Touch Bar") {
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
                    if k.key() == Key::Enter as u32 && model.finger.state == FingerState::Scan {
                        model.set_finger(FingerState::Idle);
                        dirty = true;
                    }
                    if k.key() == Key::Fn as u32 {
                        effects.extend(model.set_fn(k.key_state() == KeyState::Pressed));
                        dirty = true;
                    }
                    continue;
                }
                Event::Touch(te) => {
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
                    if peer_allowed(&stream) && stream.set_nonblocking(true).is_ok() {
                        let mut hello = serde_json::to_vec(&Outgoing::Hello).unwrap_or_default();
                        hello.push(b'\n');
                        let mut stream = stream;
                        let _ = stream.write_all(&hello);
                        clients.push(Client { stream, buf: vec![], is_agent: false });
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
        let mut agent_gone = false;
        let mut i = 0;
        while i < clients.len() {
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
            while let Some(pos) = clients[i].buf.iter().position(|b| *b == b'\n') {
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
                                effects.extend(model.set_layout(l));
                            }
                            Incoming::State { values } => model.update_state(values),
                            Incoming::Fingerprint { s } => model.set_finger(s),
                            Incoming::Layer { name } => effects.extend(model.switch_layer(&name)),
                        }
                    }
                    Err(e) => eprintln!("bad message: {e}"),
                }
            }
            if closed || clients[i].buf.len() > 1 << 20 {
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
    {
        let c = cairo::Context::new(&surf)?;
        render::draw(&model, &c, Instant::now() + Duration::from_secs_f64(age));
    }
    surf.write_to_png(&mut File::create(out)?)?;
    Ok(())
}
