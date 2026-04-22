//! Frame → terminal ASCII renderer.
//!
//! Per-cell pipeline:
//!   1. Average RGB over a source block.
//!   2. Apply user brightness offset.
//!   3. Apply style transform (Van Gogh, Mushroom, etc.).
//!   4. Recompute luma from the stylized RGB so the glyph matches the palette.
//!   5. Contrast-shape luma → glyph via `ascii::luma_to_char`.
//!   6. Emit fg escape (truecolor/256/ansi16 per `depth`) + glyph (unless style is B&W).
//!
//! One preallocated String is reused per frame; we write it to stdout in
//! a single call.

use crate::{
    ascii,
    camera::Frame,
    color::{self, ColorDepth, Fg},
    style::{self, Style, StyleCtx},
};
use std::io::Write;

/// How each cell's glyph is chosen.
///
/// - `Ascii`: density-ramped ASCII glyph — luma picks from 95-char ramp;
///   shape is the primary signal (though fg color still varies).
/// - `Blocks`: solid `█` (U+2588) in color styles, so *color* alone carries
///   the image. In B&W style we fall back to the `░▒▓█` shading ramp
///   because a solid block with no color would be information-free.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RenderMode {
    Ascii,
    Blocks,
}

pub const MODE_CYCLE: [RenderMode; 2] = [RenderMode::Ascii, RenderMode::Blocks];

impl RenderMode {
    pub fn label(self) -> &'static str {
        match self {
            RenderMode::Ascii => "ASCII",
            RenderMode::Blocks => "Blocks",
        }
    }

    pub fn cycle(self, dir: i32) -> Self {
        let idx = MODE_CYCLE.iter().position(|&m| m == self).unwrap_or(0) as i32;
        let n = MODE_CYCLE.len() as i32;
        MODE_CYCLE[((idx + dir).rem_euclid(n)) as usize]
    }
}

#[derive(Clone, Copy)]
pub struct RenderConfig {
    pub style: Style,
    pub mode: RenderMode,
    pub depth: ColorDepth,
    /// What `ColorDepth::Auto` resolves to — detected once at startup.
    pub detected: ColorDepth,
    pub brightness: f32, // -1.0..=1.0, added as (brightness * 255) per channel
    pub contrast: f32,
    pub mirror: bool,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            style: Style::Color,
            mode: RenderMode::Ascii,
            depth: ColorDepth::Auto,
            detected: ColorDepth::Palette256,
            brightness: 0.0,
            contrast: 1.2,
            mirror: true,
        }
    }
}

impl RenderConfig {
    pub fn effective_depth(&self) -> ColorDepth {
        self.depth.resolve(self.detected)
    }
}

/// Approximate terminal cell height-to-width ratio. Most fonts land near 2.0;
/// tweak here if your terminal renders tall (more like 2.2) or square (1.6).
const CHAR_ASPECT: f32 = 2.0;

pub fn render(
    frame: &Frame,
    cols: u16,
    rows: u16,
    cfg: &RenderConfig,
    time: f32,
    out: &mut String,
) {
    out.clear();
    // Begin synchronized update (DEC 2026). Supporting terminals (iTerm2,
    // Kitty, WezTerm, Ghostty, Alacritty) hold the frame until the matching
    // end marker, preventing intra-frame tearing. Others ignore the escape.
    out.push_str("\x1b[?2026h\x1b[H");

    let (fw, fh) = (frame.width as usize, frame.height as usize);
    if fw == 0 || fh == 0 || cols == 0 || rows == 0 {
        return;
    }

    // Crop the source so its aspect matches the terminal grid's pixel canvas.
    // Canvas pixel aspect is cols : rows * CHAR_ASPECT (cells are tall, not square).
    let src_ar = fw as f32 / fh as f32;
    let canvas_ar = cols as f32 / (rows as f32 * CHAR_ASPECT);
    let (crop_w, crop_h) = if src_ar > canvas_ar {
        (fh as f32 * canvas_ar, fh as f32) // source too wide → crop L/R
    } else {
        (fw as f32, fw as f32 / canvas_ar) // source too tall → crop T/B
    };
    let crop_x0 = (fw as f32 - crop_w) * 0.5;
    let crop_y0 = (fh as f32 - crop_h) * 0.5;

    let cell_w = crop_w / cols as f32;
    let cell_h = crop_h / rows as f32;
    let brightness_offset = (cfg.brightness * 255.0) as i32;
    let emit_color = cfg.style.emits_color();
    let depth = cfg.effective_depth();

    let mut last_fg: Option<Fg> = None;

    for r in 0..rows {
        let y0 = (crop_y0 + r as f32 * cell_h) as usize;
        let y1 = (crop_y0 + (r + 1) as f32 * cell_h).min(fh as f32) as usize;

        for c in 0..cols {
            let cx = if cfg.mirror { cols - 1 - c } else { c };
            let x0 = (crop_x0 + cx as f32 * cell_w) as usize;
            let x1 = (crop_x0 + (cx + 1) as f32 * cell_w).min(fw as f32) as usize;

            let (mut sr, mut sg, mut sb, mut n) = (0u64, 0u64, 0u64, 0u64);
            for y in y0..y1 {
                let row_off = y * fw * 3;
                for x in x0..x1 {
                    let i = row_off + x * 3;
                    sr += frame.rgb[i] as u64;
                    sg += frame.rgb[i + 1] as u64;
                    sb += frame.rgb[i + 2] as u64;
                    n += 1;
                }
            }
            if n == 0 {
                out.push(' ');
                continue;
            }

            let avg_r = contrast_u8(
                clamp_u8(((sr / n) as i32) + brightness_offset),
                cfg.contrast,
            );
            let avg_g = contrast_u8(
                clamp_u8(((sg / n) as i32) + brightness_offset),
                cfg.contrast,
            );
            let avg_b = contrast_u8(
                clamp_u8(((sb / n) as i32) + brightness_offset),
                cfg.contrast,
            );

            let ctx = StyleCtx { time, x: c, y: r, cols, rows };
            let (sr8, sg8, sb8) = style::transform(cfg.style, (avg_r, avg_g, avg_b), &ctx);

            let luma =
                (0.299 * sr8 as f32 + 0.587 * sg8 as f32 + 0.114 * sb8 as f32) / 255.0;
            let ch = match cfg.mode {
                RenderMode::Ascii => ascii::luma_to_char(luma, cfg.contrast, emit_color),
                // Color styles: color escapes carry brightness, so always solid █.
                // B&W: no color, fall back to shade ramp so the image is legible.
                RenderMode::Blocks if emit_color => '█',
                RenderMode::Blocks => ascii::luma_to_shade(luma, cfg.contrast),
            };

            if emit_color {
                let fg = color::quantize(depth, sr8, sg8, sb8);
                if last_fg != Some(fg) {
                    fg.write(out);
                    last_fg = Some(fg);
                }
            }
            out.push(ch);
        }

        out.push_str("\x1b[0m");
        if r < rows - 1 {
            out.push_str("\r\n");
        }
        last_fg = None;
    }
}

fn clamp_u8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// Linear contrast stretch around mid-gray (128). Compounds with the
/// glyph-density contrast in `ascii::luma_to_char` — the slider hits both
/// the RGB emitted to the terminal and the glyph shape, so turning it up
/// feels like an actual photo-editing contrast control rather than a
/// subtle glyph-ramp tweak.
fn contrast_u8(v: u8, c: f32) -> u8 {
    let n = v as f32 / 255.0;
    let stretched = ((n - 0.5) * c + 0.5).clamp(0.0, 1.0);
    (stretched * 255.0) as u8
}

pub fn flush(buf: &str) -> std::io::Result<()> {
    let mut out = std::io::stdout().lock();
    out.write_all(buf.as_bytes())?;
    // End synchronized update — the frame (including any menu overlay
    // appended after render) is now released to the terminal as one paint.
    out.write_all(b"\x1b[?2026l")?;
    out.flush()
}
