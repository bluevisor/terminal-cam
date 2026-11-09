//! Animated GIF recording of the styled feed.
//!
//! The render loop rasterizes frames (same look as screenshots) at up to
//! `RECORD_FPS` and hands them to a background thread that quantizes and
//! encodes them, so recording never stalls the live view. If the encoder
//! falls behind, new frames are skipped and the previous frame is simply
//! shown longer — playback stays real-time.

use crate::{camera::Frame, render, screenshot};
use anyhow::{Context, Result};
use std::{
    fs::{self, File},
    io::BufWriter,
    path::{Path, PathBuf},
    sync::mpsc::{sync_channel, SyncSender, TrySendError},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const RECORD_FPS: u32 = 15;
/// Recording grids wider than this are scaled down (keeping the terminal's
/// aspect) so a full-screen terminal doesn't produce a 3000 px GIF.
const MAX_COLS: u16 = 120;
/// Auto-stop so a forgotten recording can't fill the disk.
pub const MAX_DURATION: Duration = Duration::from_secs(60);
/// NeuQuant sampling factor (1 = best/slowest, 30 = fastest).
const QUANT_SPEED: i32 = 10;

struct GifFrame {
    rgb: Vec<u8>,
    taken: Instant,
}

pub struct Recorder {
    tx: SyncSender<(Vec<u8>, u16)>,
    worker: JoinHandle<Result<Saved>>,
    cols: u16,
    rows: u16,
    state: render::RenderState,
    raster: render::StreamRaster,
    started: Instant,
    pending: Option<GifFrame>,
}

/// A recording that is still flushing in the background.
pub struct Finishing(JoinHandle<Result<Saved>>);

pub struct Saved {
    pub path: PathBuf,
    pub frames: usize,
}

impl Recorder {
    pub fn start(dir: &Path, term_cols: u16, term_rows: u16) -> Result<Self> {
        let (cols, rows) = record_grid(term_cols, term_rows);
        let (width, height) = render::stream_pixel_size(cols, rows);

        fs::create_dir_all(dir)
            .with_context(|| format!("create recording directory {}", dir.display()))?;
        let path = dir.join(screenshot::timestamped_name("gif")?);
        let file =
            File::create(&path).with_context(|| format!("create recording {}", path.display()))?;
        let mut encoder = gif::Encoder::new(BufWriter::new(file), width as u16, height as u16, &[])
            .context("start GIF encoder")?;
        encoder
            .set_repeat(gif::Repeat::Infinite)
            .context("set GIF looping")?;

        let (tx, rx) = sync_channel::<(Vec<u8>, u16)>(4);
        let worker = thread::spawn(move || -> Result<Saved> {
            let mut frames = 0;
            for (rgb, delay) in rx {
                let mut frame =
                    gif::Frame::from_rgb_speed(width as u16, height as u16, &rgb, QUANT_SPEED);
                frame.delay = delay;
                encoder.write_frame(&frame).context("write GIF frame")?;
                frames += 1;
            }
            drop(encoder); // writes the trailer and flushes
            Ok(Saved { path, frames })
        });

        Ok(Self {
            tx,
            worker,
            cols,
            rows,
            state: render::RenderState::new(),
            raster: render::StreamRaster::new(),
            started: Instant::now(),
            pending: None,
        })
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Captures a frame if one is due. Returns false once `MAX_DURATION`
    /// is reached; the caller should then `stop()`.
    pub fn tick(&mut self, frame: &Frame, cfg: &render::RenderConfig, time: f32) -> bool {
        if self.elapsed() >= MAX_DURATION {
            return false;
        }
        let interval = Duration::from_secs(1) / RECORD_FPS;
        let now = Instant::now();
        if self
            .pending
            .as_ref()
            .is_some_and(|p| now.duration_since(p.taken) < interval)
        {
            return true;
        }

        if !render::render_stream_bgra(
            frame,
            self.cols,
            self.rows,
            cfg,
            time,
            &mut self.state,
            &mut self.raster,
        ) {
            return true;
        }
        let rgb = bgra_to_rgb(&self.raster.bgra);

        // The pending frame's display time ends now that a newer one exists.
        match self.pending.take() {
            None => self.pending = Some(GifFrame { rgb, taken: now }),
            Some(prev) => {
                let delay = centis(now.duration_since(prev.taken));
                match self.tx.try_send((prev.rgb, delay)) {
                    Ok(()) => self.pending = Some(GifFrame { rgb, taken: now }),
                    // Encoder is behind: keep showing the older frame longer.
                    Err(TrySendError::Full((rgb, _))) => {
                        self.pending = Some(GifFrame {
                            rgb,
                            taken: prev.taken,
                        })
                    }
                    Err(TrySendError::Disconnected(_)) => return false,
                }
            }
        }
        true
    }

    pub fn stop(mut self) -> Finishing {
        if let Some(last) = self.pending.take() {
            let delay = centis(Duration::from_secs(1) / RECORD_FPS);
            let _ = self.tx.send((last.rgb, delay));
        }
        drop(self.tx);
        Finishing(self.worker)
    }
}

impl Finishing {
    pub fn is_done(&self) -> bool {
        self.0.is_finished()
    }

    pub fn wait(self) -> Result<Saved> {
        self.0
            .join()
            .unwrap_or_else(|_| Err(anyhow::anyhow!("GIF encoder thread panicked")))
    }
}

/// Terminal grid, shrunk to at most `MAX_COLS` wide with the same aspect.
fn record_grid(cols: u16, rows: u16) -> (u16, u16) {
    let (cols, rows) = (cols.max(1), rows.max(1));
    if cols <= MAX_COLS {
        return (cols, rows);
    }
    let scaled_rows = (rows as u32 * MAX_COLS as u32 + cols as u32 / 2) / cols as u32;
    (MAX_COLS, scaled_rows.max(1) as u16)
}

/// GIF delays are in hundredths of a second; never 0 (browsers treat
/// 0–1 as "as fast as possible" and some bump it to 10).
fn centis(d: Duration) -> u16 {
    ((d.as_millis() + 5) / 10).clamp(2, u16::MAX as u128) as u16
}

fn bgra_to_rgb(bgra: &[u8]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(bgra.len() / 4 * 3);
    for px in bgra.chunks_exact(4) {
        rgb.extend_from_slice(&[px[2], px[1], px[0]]);
    }
    rgb
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_terminals_are_capped_keeping_aspect() {
        assert_eq!(record_grid(80, 24), (80, 24));
        assert_eq!(record_grid(240, 60), (120, 30));
        assert_eq!(record_grid(0, 0), (1, 1));
    }

    #[test]
    fn delays_round_to_centiseconds() {
        assert_eq!(centis(Duration::from_millis(66)), 7);
        assert_eq!(centis(Duration::from_millis(1)), 2);
    }

    #[test]
    fn records_a_readable_gif() {
        let dir = std::env::temp_dir().join(format!("tc-rec-test-{}", std::process::id()));
        let frame = Frame {
            width: 64,
            height: 48,
            rgb: vec![120; 64 * 48 * 3],
        };
        let cfg = render::RenderConfig::default();
        let mut rec = Recorder::start(&dir, 8, 4).unwrap();
        for i in 0..3 {
            if let Some(p) = rec.pending.as_mut() {
                p.taken -= Duration::from_millis(100);
            }
            assert!(rec.tick(&frame, &cfg, i as f32));
        }
        let saved = rec.stop().wait().unwrap();
        assert_eq!(saved.frames, 3);
        let bytes = fs::read(&saved.path).unwrap();
        assert!(bytes.starts_with(b"GIF89a"));
        let _ = fs::remove_dir_all(dir);
    }
}
