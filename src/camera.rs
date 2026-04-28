//! Camera enumeration + capture thread.
//!
//! Uses nokhwa (AVFoundation on macOS). The capture thread publishes the
//! latest decoded RGB frame into a shared slot — we don't buffer; stale frames
//! are overwritten. The render loop reads whatever's freshest.

use anyhow::{anyhow, Result};
use nokhwa::{
    pixel_format::RgbFormat,
    query,
    utils::{ApiBackend, CameraIndex, RequestedFormat, RequestedFormatType},
    Camera,
};
use parking_lot::Mutex;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::Duration;

/// Captures the most recent capture-thread error so the UI can show it
/// instead of an indefinite "waiting for camera…" hang. Wrapped in `Arc`
/// so the spawned thread can write it from any scope.
pub type SharedError = Arc<Mutex<Option<String>>>;

#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct CameraInfo {
    pub index: u32,
    pub name: String,
}

pub fn list_cameras() -> Result<Vec<CameraInfo>> {
    let infos = query(ApiBackend::Auto).map_err(|e| anyhow!("nokhwa query: {e}"))?;
    Ok(infos
        .into_iter()
        .enumerate()
        .map(|(i, c)| CameraInfo {
            index: i as u32,
            name: c.human_name(),
        })
        .collect())
}

pub type SharedFrame = Arc<Mutex<Option<Frame>>>;

pub struct CaptureHandle {
    pub frame: SharedFrame,
    pub error: SharedError,
    running: Arc<AtomicBool>,
}

impl CaptureHandle {
    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn spawn_capture(camera_index: u32) -> Result<CaptureHandle> {
    let frame: SharedFrame = Arc::new(Mutex::new(None));
    let error: SharedError = Arc::new(Mutex::new(None));
    let running = Arc::new(AtomicBool::new(true));

    let frame_c = frame.clone();
    let error_c = error.clone();
    let running_c = running.clone();

    thread::spawn(move || {
        let index = CameraIndex::Index(camera_index);
        let requested =
            RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate);
        let mut camera = match Camera::new(index, requested) {
            Ok(c) => c,
            Err(e) => {
                *error_c.lock() = Some(format!("camera open failed: {e}"));
                return;
            }
        };
        if let Err(e) = camera.open_stream() {
            *error_c.lock() = Some(format!("camera stream open failed: {e}"));
            return;
        }

        while running_c.load(Ordering::Relaxed) {
            match camera.frame() {
                Ok(buf) => {
                    if let Ok(img) = buf.decode_image::<RgbFormat>() {
                        let (w, h) = (img.width(), img.height());
                        let rgb = img.into_raw();
                        *frame_c.lock() = Some(Frame { width: w, height: h, rgb });
                    }
                }
                Err(_) => thread::sleep(Duration::from_millis(5)),
            }
        }

        let _ = camera.stop_stream();
    });

    Ok(CaptureHandle { frame, error, running })
}
