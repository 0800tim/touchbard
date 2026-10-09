//! The Touch Bar's panel (DRM), its backlight, and the virtual keyboard that
//! turns touches into key presses.

use anyhow::{anyhow, bail, Context as _, Result};
use drm::buffer::DrmFourcc;
use drm::buffer::Buffer as _;
use drm::control::{connector, crtc, dumbbuffer::DumbBuffer, framebuffer, ClipRect, Device as ControlDevice, Mode};
use drm::Device;
use input_linux::{EventKind, Key, SynchronizeKind, UInputHandle};
use input_linux_sys::{input_event, input_id, timeval, uinput_setup};
use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;

pub struct Card(File);

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}
impl Device for Card {}
impl ControlDevice for Card {}

pub struct Output {
    card: Card,
    db: DumbBuffer,
    fb: framebuffer::Handle,
    crtc: crtc::Handle,
    conn: connector::Handle,
    mode: Mode,
    /// Panel size as the hardware sees it: narrow and tall (60 x 2008).
    pub dev_w: u32,
    pub dev_h: u32,
}

impl Output {
    /// Find the Touch Bar: the card whose connected panel is a tall sliver.
    pub fn open() -> Result<Output> {
        let mut last = anyhow!("no Touch Bar panel found");
        for attempt in 0..20 {
            for entry in fs::read_dir("/dev/dri")?.flatten() {
                let path = entry.path();
                if !path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("card")) {
                    continue;
                }
                match Self::try_card(path.clone()) {
                    Ok(Some(o)) => return Ok(o),
                    Ok(None) => {}
                    Err(e) => last = e.context(format!("{}", path.display())),
                }
            }
            // The previous owner may still be letting go of the card.
            if attempt < 19 {
                sleep(Duration::from_millis(250));
            }
        }
        Err(last)
    }

    fn try_card(path: PathBuf) -> Result<Option<Output>> {
        let card = Card(OpenOptions::new().read(true).write(true).open(&path)?);
        let res = card.resource_handles()?;
        for &conn in res.connectors() {
            let info = card.get_connector(conn, true)?;
            if info.state() != connector::State::Connected {
                continue;
            }
            let Some(&mode) = info.modes().first() else { continue };
            let (w, h) = mode.size();
            if (h as u32) < (w as u32) * 8 {
                continue;
            }
            // Only now, knowing this is the Touch Bar, take control of it.
            let _ = card.acquire_master_lock();
            let crtc = info
                .current_encoder()
                .and_then(|e| card.get_encoder(e).ok())
                .and_then(|e| e.crtc())
                .or_else(|| {
                    info.encoders().iter().find_map(|&e| {
                        let enc = card.get_encoder(e).ok()?;
                        res.filter_crtcs(enc.possible_crtcs()).first().copied()
                    })
                })
                .context("no CRTC for the Touch Bar")?;
            // The display engine reads rows padded to 64 bytes, whatever pitch the
            // kernel reports for a 60 px buffer (240 bytes gives a sheared image).
            // Allocate 64 px wide and draw into the first 60, as tiny-dfr does.
            let alloc_w = (w as u32 + 15) & !15;
            let db = card.create_dumb_buffer((alloc_w, h as u32), DrmFourcc::Xrgb8888, 32)?;
            let fb = card.add_framebuffer(&db, 24, 32)?;
            card.set_crtc(crtc, Some(fb), (0, 0), &[conn], Some(mode))
                .context("set_crtc (is another Touch Bar daemon still running?)")?;
            if let Ok(fi) = card.get_framebuffer(fb) {
                eprintln!(
                    "touchbard: {} mode {}x{}, dumb {:?} pitch {} len {}, fb {:?} pitch {} bpp {}",
                    path.display(), w, h, db.size(), db.pitch(), db.size().0 * 4, fi.size(), fi.pitch(), fi.bpp()
                );
            }
            return Ok(Some(Output { card, db, fb, crtc, conn, mode, dev_w: w as u32, dev_h: h as u32 }));
        }
        Ok(None)
    }

    /// Landscape size, the coordinate space everything is drawn and touched in.
    pub fn size(&self) -> (u32, u32) {
        (self.dev_h, self.dev_w)
    }

    /// Copy a landscape frame onto the portrait panel, rotating as we go.
    pub fn present(&mut self, frame: &mut cairo::ImageSurface) -> Result<()> {
        let (lw, lh) = (frame.width() as usize, frame.height() as usize);
        let lstride = frame.stride() as usize / 4;
        let pitch = self.db.pitch() as usize / 4;
        let (dw, dh) = (self.dev_w as usize, self.dev_h as usize);
        {
            let src = frame.data().map_err(|e| anyhow!("{e}"))?;
            let src: &[u32] = bytemuck_cast(&src);
            let mut map = self.card.map_dumb_buffer(&mut self.db)?;
            let dst: &mut [u32] = bytemuck_cast_mut(map.as_mut());
            for dy in 0..dh.min(lw) {
                let row = dy * pitch;
                for dx in 0..dw.min(lh) {
                    // Device column dx shows landscape row (height - 1 - dx).
                    dst[row + dx] = src[(lh - 1 - dx) * lstride + dy];
                }
            }
        }
        match self.card.dirty_framebuffer(self.fb, &[ClipRect::new(0, 0, self.dev_w as u16, self.dev_h as u16)]) {
            Ok(()) => Ok(()),
            // Some drivers scan out continuously and don't implement dirty.
            Err(e) if e.raw_os_error() == Some(libc::ENOSYS) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// After a suspend or a lost master, put our framebuffer back on screen.
    pub fn restore(&self) -> Result<()> {
        let _ = self.card.acquire_master_lock();
        self.card.set_crtc(self.crtc, Some(self.fb), (0, 0), &[self.conn], Some(self.mode))?;
        Ok(())
    }
}

fn bytemuck_cast(b: &[u8]) -> &[u32] {
    assert!(b.as_ptr() as usize % 4 == 0);
    unsafe { std::slice::from_raw_parts(b.as_ptr() as *const u32, b.len() / 4) }
}

fn bytemuck_cast_mut(b: &mut [u8]) -> &mut [u32] {
    assert!(b.as_ptr() as usize % 4 == 0);
    unsafe { std::slice::from_raw_parts_mut(b.as_mut_ptr() as *mut u32, b.len() / 4) }
}

// ---- backlight -------------------------------------------------------------

fn read_u32(p: PathBuf) -> Option<u32> {
    fs::read_to_string(p).ok()?.trim().parse().ok()
}

pub struct Backlight {
    tb: File,
    pub tb_max: u32,
    display: Option<PathBuf>,
    current: Option<u32>,
}

impl Backlight {
    pub fn open() -> Result<Backlight> {
        let mut tb = None;
        let mut display = None;
        for e in fs::read_dir("/sys/class/backlight")?.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.contains("dsi") || name.contains("display-pipe") || name == "appletb_backlight" {
                tb = Some(e.path());
            } else if ["apple-panel-bl", "gmux_backlight", "intel_backlight", "acpi_video0"].contains(&name.as_str()) {
                display = Some(e.path());
            }
        }
        let tb = tb.context("no Touch Bar backlight")?;
        let tb_max = read_u32(tb.join("max_brightness")).unwrap_or(255);
        let file = OpenOptions::new().write(true).open(tb.join("brightness"))?;
        Ok(Backlight { tb: file, tb_max, display, current: None })
    }

    /// Main display brightness, 0..1. 1 when there is no display backlight.
    pub fn display_level(&self) -> f64 {
        let Some(d) = &self.display else { return 1.0 };
        match (read_u32(d.join("brightness")), read_u32(d.join("max_brightness"))) {
            (Some(b), Some(m)) if m > 0 => b as f64 / m as f64,
            _ => 1.0,
        }
    }

    /// `v` is on a 0..=255 scale. Scale it to the device's range rather than clamping:
    /// Apple Silicon Touch Bars take 0..=255, but Intel T2 ones (`appletb_backlight`)
    /// only 0..=2 (off, dim, full). Round up so any non-zero level stays visible and
    /// the dimmed state (a quarter of full) still differs from full.
    pub fn set(&mut self, v: u32) {
        let v = v.min(255);
        let v = (v * self.tb_max).div_ceil(255);
        if self.current == Some(v) {
            return;
        }
        let ok = self.tb.seek(SeekFrom::Start(0)).is_ok() && self.tb.write_all(format!("{v}\n").as_bytes()).is_ok();
        if ok {
            self.current = Some(v);
        }
    }

    pub fn is_off(&self) -> bool {
        self.current == Some(0)
    }
}

// ---- virtual keyboard ------------------------------------------------------

pub struct Keyboard(UInputHandle<File>);

impl Keyboard {
    pub fn open() -> Result<Keyboard> {
        let u = UInputHandle::new(OpenOptions::new().write(true).open("/dev/uinput")?);
        u.set_evbit(EventKind::Key)?;
        for code in 1..0x2ff {
            if let Ok(k) = Key::from_code(code) {
                let _ = u.set_keybit(k);
            }
        }
        let mut name = [0 as libc::c_char; 80];
        for (i, b) in b"Touch Bar Virtual Keyboard".iter().enumerate() {
            name[i] = *b as libc::c_char;
        }
        u.dev_setup(&uinput_setup {
            id: input_id { bustype: 0x19, vendor: 0x1209, product: 0x316e, version: 2 },
            ff_effects_max: 0,
            name,
        })?;
        u.dev_create()?;
        Ok(Keyboard(u))
    }

    fn emit(&self, kind: EventKind, code: u16, value: i32) -> Result<()> {
        let ev = input_event { time: timeval { tv_sec: 0, tv_usec: 0 }, type_: kind as u16, code, value };
        if self.0.write(&[ev])? != 1 {
            bail!("short uinput write");
        }
        Ok(())
    }

    pub fn keys(&self, keys: &[Key], down: bool) {
        // Press a chord in order, release it in reverse.
        let order: Vec<&Key> = if down { keys.iter().collect() } else { keys.iter().rev().collect() };
        for k in order {
            let _ = self.emit(EventKind::Key, *k as u16, down as i32);
        }
        let _ = self.emit(EventKind::Synchronize, SynchronizeKind::Report as u16, 0);
    }
}
