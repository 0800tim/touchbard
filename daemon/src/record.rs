//! Screen recording. The Touch Bar isn't a Wayland output, so no screen
//! recorder can see it; instead a client asks for `{"t":"record"}` and gets a
//! memfd holding the latest frame, which it samples at its own frame rate
//! (`touchbar-agent record` feeds it to ffmpeg).
//!
//! Layout: a 64-byte header (`HEADER` below), then BGRA rows. `seq` is a
//! seqlock: odd while a frame is being written, so readers retry.

use anyhow::{bail, Result};
use std::collections::HashMap;
use std::io::IoSlice;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};

const HEADER: usize = 64; // u32 magic "TBR1", w, h, stride; u64 seq at 16
const MAGIC: u32 = u32::from_le_bytes(*b"TBR1");

pub struct Recorder {
    fd: OwnedFd,
    ptr: *mut u8,
    len: usize,
    surface: cairo::ImageSurface,
}

impl Recorder {
    pub fn new(w: i32, h: i32) -> Result<Recorder> {
        let stride = cairo::Format::ARgb32.stride_for_width(w as u32)?;
        let len = HEADER + (stride * h) as usize;
        unsafe {
            let raw = libc::memfd_create(c"touchbar-record".as_ptr(), libc::MFD_CLOEXEC);
            if raw < 0 {
                bail!("memfd_create: {}", std::io::Error::last_os_error());
            }
            let fd = OwnedFd::from_raw_fd(raw);
            if libc::ftruncate(raw, len as _) != 0 {
                bail!("ftruncate: {}", std::io::Error::last_os_error());
            }
            let ptr = libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, raw, 0);
            if ptr == libc::MAP_FAILED {
                bail!("mmap: {}", std::io::Error::last_os_error());
            }
            let ptr = ptr as *mut u8;
            for (i, v) in [MAGIC, w as u32, h as u32, stride as u32].iter().enumerate() {
                (ptr as *mut u32).add(i).write(*v);
            }
            let surface = cairo::ImageSurface::create_for_data_unsafe(ptr.add(HEADER), cairo::Format::ARgb32, w, h, stride)?;
            Ok(Recorder { fd, ptr, len, surface })
        }
    }

    /// Hand the memfd to the client, with a line saying what it is.
    pub fn send(&self, stream: &UnixStream) -> Result<()> {
        use nix::sys::socket::{sendmsg, ControlMessage, MsgFlags, UnixAddr};
        let (w, h) = (self.surface.width(), self.surface.height());
        let line = format!("{{\"t\":\"record\",\"w\":{w},\"h\":{h},\"offset\":{HEADER},\"stride\":{}}}\n", self.surface.stride());
        let fds = [self.fd.as_raw_fd()];
        sendmsg::<UnixAddr>(stream.as_raw_fd(), &[IoSlice::new(line.as_bytes())], &[ControlMessage::ScmRights(&fds)], MsgFlags::empty(), None)?;
        Ok(())
    }

    fn seq(&self) -> &AtomicU64 {
        unsafe { &*(self.ptr.add(16) as *const AtomicU64) }
    }

    /// Copy a finished frame in, with a soft dot under each finger.
    pub fn write(&self, frame: &cairo::ImageSurface, touches: &HashMap<u32, f64>) {
        self.seq().fetch_add(1, Ordering::AcqRel);
        if let Ok(c) = cairo::Context::new(&self.surface) {
            c.set_operator(cairo::Operator::Source);
            let _ = c.set_source_surface(frame, 0.0, 0.0);
            let _ = c.paint();
            c.set_operator(cairo::Operator::Over);
            let y = self.surface.height() as f64 / 2.0;
            for &x in touches.values() {
                let g = cairo::RadialGradient::new(x, y, 0.0, x, y, 22.0);
                g.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.55);
                g.add_color_stop_rgba(0.6, 1.0, 1.0, 1.0, 0.25);
                g.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.0);
                let _ = c.set_source(&g);
                c.arc(x, y, 22.0, 0.0, std::f64::consts::TAU);
                let _ = c.fill();
            }
        }
        self.surface.flush();
        self.seq().fetch_add(1, Ordering::AcqRel);
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.surface.finish();
        unsafe { libc::munmap(self.ptr as *mut _, self.len) };
    }
}
