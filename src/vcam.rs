//! Virtual camera output (macOS only).
//!
//! Feeds rendered frames to the "Terminal Cam" CMIO camera extension
//! (see macos/) through its sink stream. The extension must be installed
//! via TerminalCamCamera.app before the device exists; until then (and
//! whenever the device disappears) this module keeps retrying quietly.

use crate::{camera::Frame, render};
use std::time::{Duration, Instant};

/// Must match `cameraDeviceUUID` in macos/Extension/CameraExtension.swift.
const DEVICE_UID: &str = "72BC5AE8-7026-4057-90A9-A5BD0BDFDD95";

/// 160×45 cells rasterize at 8×16 px/cell to exactly 1280×720.
pub const VCAM_COLS: u16 = 160;
pub const VCAM_ROWS: u16 = 45;

const RECONNECT_INTERVAL: Duration = Duration::from_secs(3);

#[cfg(target_os = "macos")]
mod ffi {
    use std::ffi::{c_char, c_void};

    extern "C" {
        pub fn vcam_open(device_uid: *const c_char, width: i32, height: i32) -> *mut c_void;
        pub fn vcam_push(handle: *mut c_void, bgra: *const u8) -> i32;
        pub fn vcam_close(handle: *mut c_void);
    }
}

/// Connection-state transition worth telling the user about.
pub enum VcamEvent {
    /// Device found and sink stream started.
    Connected,
    /// First connection attempt failed — the extension is probably not
    /// installed or not approved yet.
    Waiting,
    /// A previously working connection died; reconnecting.
    Disconnected,
    None,
}

pub struct VirtualCam {
    #[cfg(target_os = "macos")]
    handle: *mut std::ffi::c_void,
    last_attempt: Option<Instant>,
    announced_waiting: bool,
    state: render::RenderState,
    raster: render::StreamRaster,
}

impl VirtualCam {
    pub fn new() -> Self {
        Self {
            #[cfg(target_os = "macos")]
            handle: std::ptr::null_mut(),
            last_attempt: None,
            announced_waiting: false,
            state: render::RenderState::new(),
            raster: render::StreamRaster::new(),
        }
    }

    #[cfg(target_os = "macos")]
    pub fn push(&mut self, frame: &Frame, cfg: &render::RenderConfig, time: f32) -> VcamEvent {
        let mut event = VcamEvent::None;

        if self.handle.is_null() {
            if self
                .last_attempt
                .is_some_and(|at| at.elapsed() < RECONNECT_INTERVAL)
            {
                return VcamEvent::None;
            }
            self.last_attempt = Some(Instant::now());

            let uid = std::ffi::CString::new(DEVICE_UID).expect("static UID has no NUL");
            let (width, height) = render::stream_pixel_size(VCAM_COLS, VCAM_ROWS);
            self.handle = unsafe { ffi::vcam_open(uid.as_ptr(), width as i32, height as i32) };

            if self.handle.is_null() {
                if !self.announced_waiting {
                    self.announced_waiting = true;
                    return VcamEvent::Waiting;
                }
                return VcamEvent::None;
            }
            event = VcamEvent::Connected;
        }

        if !render::render_stream_bgra(
            frame,
            VCAM_COLS,
            VCAM_ROWS,
            cfg,
            time,
            &mut self.state,
            &mut self.raster,
        ) {
            return event;
        }

        let rc = unsafe { ffi::vcam_push(self.handle, self.raster.bgra.as_ptr()) };
        if rc < 0 {
            unsafe { ffi::vcam_close(self.handle) };
            self.handle = std::ptr::null_mut();
            return VcamEvent::Disconnected;
        }

        event
    }

    #[cfg(not(target_os = "macos"))]
    pub fn push(&mut self, _frame: &Frame, _cfg: &render::RenderConfig, _time: f32) -> VcamEvent {
        if !self.announced_waiting {
            self.announced_waiting = true;
            let _ = &self.last_attempt;
            let _ = (&self.state, &self.raster);
            return VcamEvent::Waiting;
        }
        VcamEvent::None
    }
}

#[cfg(target_os = "macos")]
impl Drop for VirtualCam {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { ffi::vcam_close(self.handle) };
        }
    }
}
