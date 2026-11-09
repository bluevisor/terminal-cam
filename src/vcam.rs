//! Virtual camera output (macOS only).
//!
//! Feeds rendered frames to the "Terminal Cam" CMIO camera extension
//! (see macos/) through its sink stream. The extension must be installed
//! via TerminalCamCamera.app before the device exists; until then (and
//! whenever the device disappears) this module keeps retrying quietly.
//!
//! On other platforms `VirtualCam` is a stub that reports "macOS only", so
//! most of this module is intentionally unused there.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use crate::{camera::Frame, render};
use std::time::{Duration, Instant};

/// Must match `cameraDeviceUUID` in macos/Extension/CameraExtension.swift.
const DEVICE_UID: &str = "72BC5AE8-7026-4057-90A9-A5BD0BDFDD95";

/// The extension publishes one fixed format; every detail level is
/// resampled to it.
pub const OUT_WIDTH: usize = 1280;
pub const OUT_HEIGHT: usize = 720;

/// Cell grid the virtual camera renders before scaling to 1280×720. Every
/// grid rasterizes (8×16 px cells) to an exact 16:9 image, so scaling never
/// crops or letterboxes. Fewer cells = bigger, bolder glyphs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VcamDetail {
    Chunky,
    Coarse,
    Normal,
    Fine,
}

pub const DETAIL_CYCLE: [VcamDetail; 4] = [
    VcamDetail::Chunky,
    VcamDetail::Coarse,
    VcamDetail::Normal,
    VcamDetail::Fine,
];

impl VcamDetail {
    pub fn grid(self) -> (u16, u16) {
        match self {
            VcamDetail::Chunky => (64, 18),
            VcamDetail::Coarse => (96, 27),
            VcamDetail::Normal => (160, 45), // native: exactly 1280×720
            VcamDetail::Fine => (224, 63),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            VcamDetail::Chunky => "chunky",
            VcamDetail::Coarse => "coarse",
            VcamDetail::Normal => "normal",
            VcamDetail::Fine => "fine",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        crate::names::find(&DETAIL_CYCLE, name, |d| [d.label(), d.label()])
    }

    pub fn cycle(self, dir: i32) -> Self {
        let idx = DETAIL_CYCLE.iter().position(|&d| d == self).unwrap_or(0) as i32;
        let n = DETAIL_CYCLE.len() as i32;
        DETAIL_CYCLE[((idx + dir).rem_euclid(n)) as usize]
    }
}

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
    /// Not macOS: there is no camera extension to talk to.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Unsupported,
    None,
}

pub struct VirtualCam {
    #[cfg(target_os = "macos")]
    handle: *mut std::ffi::c_void,
    last_attempt: Option<Instant>,
    announced_waiting: bool,
    state: render::RenderState,
    raster: render::StreamRaster,
    /// Final 1280×720 BGRA frame when the detail grid isn't native size.
    scaled: Vec<u8>,
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
            scaled: Vec::new(),
        }
    }

    /// Short state for the menu row.
    pub fn status_label(&self) -> &'static str {
        #[cfg(target_os = "macos")]
        {
            if self.handle.is_null() {
                "searching…"
            } else {
                "connected"
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            "macOS only"
        }
    }

    #[cfg(target_os = "macos")]
    pub fn push(
        &mut self,
        frame: &Frame,
        cfg: &render::RenderConfig,
        detail: VcamDetail,
        time: f32,
    ) -> VcamEvent {
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
            self.handle =
                unsafe { ffi::vcam_open(uid.as_ptr(), OUT_WIDTH as i32, OUT_HEIGHT as i32) };

            if self.handle.is_null() {
                if !self.announced_waiting {
                    self.announced_waiting = true;
                    return VcamEvent::Waiting;
                }
                return VcamEvent::None;
            }
            event = VcamEvent::Connected;
        }

        let (cols, rows) = detail.grid();
        if !render::render_stream_bgra(
            frame,
            cols,
            rows,
            cfg,
            time,
            &mut self.state,
            &mut self.raster,
        ) {
            return event;
        }

        let (w, h) = render::stream_pixel_size(cols, rows);
        let out = if (w as usize, h as usize) == (OUT_WIDTH, OUT_HEIGHT) {
            &self.raster.bgra
        } else {
            resample_bgra(
                &self.raster.bgra,
                (w as usize, h as usize),
                &mut self.scaled,
                (OUT_WIDTH, OUT_HEIGHT),
            );
            &self.scaled
        };

        let rc = unsafe { ffi::vcam_push(self.handle, out.as_ptr()) };
        if rc < 0 {
            unsafe { ffi::vcam_close(self.handle) };
            self.handle = std::ptr::null_mut();
            return VcamEvent::Disconnected;
        }

        event
    }

    #[cfg(not(target_os = "macos"))]
    pub fn push(
        &mut self,
        _frame: &Frame,
        _cfg: &render::RenderConfig,
        _detail: VcamDetail,
        _time: f32,
    ) -> VcamEvent {
        if !self.announced_waiting {
            self.announced_waiting = true;
            return VcamEvent::Unsupported;
        }
        VcamEvent::None
    }
}

/// Box-filter resample of a tight BGRA image. Each output pixel averages
/// the source pixels it covers, so upscaling is nearest-neighbour (crisp
/// glyph edges) and downscaling is a proper area average (no shimmer).
pub fn resample_bgra(
    src: &[u8],
    src_size: (usize, usize),
    dst: &mut Vec<u8>,
    dst_size: (usize, usize),
) {
    let (sw, sh) = src_size;
    let (dw, dh) = dst_size;
    dst.resize(dw * dh * 4, 255);
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return;
    }
    let span = |i: usize, s: usize, d: usize| {
        let a = i * s / d;
        let b = ((i + 1) * s / d).max(a + 1).min(s);
        (a, b)
    };
    let xs: Vec<(usize, usize)> = (0..dw).map(|x| span(x, sw, dw)).collect();
    for y in 0..dh {
        let (y0, y1) = span(y, sh, dh);
        for (x, &(x0, x1)) in xs.iter().enumerate() {
            let mut acc = [0u32; 3];
            for sy in y0..y1 {
                let row = sy * sw * 4;
                for sx in x0..x1 {
                    let i = row + sx * 4;
                    acc[0] += src[i] as u32;
                    acc[1] += src[i + 1] as u32;
                    acc[2] += src[i + 2] as u32;
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            let o = (y * dw + x) * 4;
            dst[o] = (acc[0] / n) as u8;
            dst[o + 1] = (acc[1] / n) as u8;
            dst[o + 2] = (acc[2] / n) as u8;
            dst[o + 3] = 255;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_detail_grid_is_16_by_9() {
        for d in DETAIL_CYCLE {
            let (cols, rows) = d.grid();
            let (w, h) = render::stream_pixel_size(cols, rows);
            assert_eq!(w * 9, h * 16, "{d:?} → {w}×{h}");
        }
    }

    #[test]
    fn resample_preserves_flat_color_and_size() {
        let src: Vec<u8> = [10u8, 20, 30, 255].repeat(512 * 288);
        let mut dst = Vec::new();
        resample_bgra(&src, (512, 288), &mut dst, (OUT_WIDTH, OUT_HEIGHT));
        assert_eq!(dst.len(), OUT_WIDTH * OUT_HEIGHT * 4);
        assert!(dst
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [10, 20, 30, 255]));

        let src: Vec<u8> = [200u8, 100, 50, 255].repeat(1792 * 1008);
        resample_bgra(&src, (1792, 1008), &mut dst, (OUT_WIDTH, OUT_HEIGHT));
        assert!(dst
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [200, 100, 50, 255]));
    }
}
