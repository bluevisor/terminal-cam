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
use ab_glyph::{point, Font, FontArc, ScaleFont};
use std::{fs, io::Write, sync::OnceLock};

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

pub struct ScreenshotImage {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
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
const SCREENSHOT_CELL_WIDTH: usize = 8;
const SCREENSHOT_CELL_HEIGHT: usize = 16;
const SCREENSHOT_FONT_SCALE: f32 = 15.0;
const SCREENSHOT_GLYPH_GAMMA: f32 = 0.58;
const SCREENSHOT_DEFAULT_FG: (u8, u8, u8) = (230, 230, 230);

struct RenderGeometry {
    cols: u16,
    rows: u16,
    fw: usize,
    fh: usize,
    crop_x0: f32,
    crop_y0: f32,
    cell_w: f32,
    cell_h: f32,
}

struct RenderedCell {
    rgb: (u8, u8, u8),
    ch: char,
    emit_color: bool,
}

impl RenderGeometry {
    fn new(frame: &Frame, cols: u16, rows: u16) -> Option<Self> {
        let (fw, fh) = (frame.width as usize, frame.height as usize);
        if fw == 0 || fh == 0 || cols == 0 || rows == 0 {
            return None;
        }

        // Crop the source so its aspect matches the terminal grid's pixel canvas.
        // Canvas pixel aspect is cols : rows * CHAR_ASPECT (cells are tall, not square).
        let src_ar = fw as f32 / fh as f32;
        let canvas_ar = cols as f32 / (rows as f32 * CHAR_ASPECT);
        let (crop_w, crop_h) = if src_ar > canvas_ar {
            (fh as f32 * canvas_ar, fh as f32) // source too wide -> crop L/R
        } else {
            (fw as f32, fw as f32 / canvas_ar) // source too tall -> crop T/B
        };
        let crop_x0 = (fw as f32 - crop_w) * 0.5;
        let crop_y0 = (fh as f32 - crop_h) * 0.5;

        Some(Self {
            cols,
            rows,
            fw,
            fh,
            crop_x0,
            crop_y0,
            cell_w: crop_w / cols as f32,
            cell_h: crop_h / rows as f32,
        })
    }
}

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

    let Some(geometry) = RenderGeometry::new(frame, cols, rows) else {
        return;
    };
    let depth = cfg.effective_depth();
    let mut last_fg: Option<Fg> = None;

    for r in 0..rows {
        for c in 0..cols {
            let Some(cell) = sample_cell(frame, &geometry, c, r, cfg, time) else {
                out.push(' ');
                continue;
            };

            if cell.emit_color {
                let fg = color::quantize(depth, cell.rgb.0, cell.rgb.1, cell.rgb.2);
                if last_fg != Some(fg) {
                    fg.write(out);
                    last_fg = Some(fg);
                }
            }
            out.push(cell.ch);
        }

        out.push_str("\x1b[0m");
        if r < rows - 1 {
            out.push_str("\r\n");
        }
        last_fg = None;
    }
}

pub fn render_screenshot(
    frame: &Frame,
    cols: u16,
    rows: u16,
    cfg: &RenderConfig,
    time: f32,
) -> Option<ScreenshotImage> {
    let geometry = RenderGeometry::new(frame, cols, rows)?;
    let width = usize::from(cols).checked_mul(SCREENSHOT_CELL_WIDTH)?;
    let height = usize::from(rows).checked_mul(SCREENSHOT_CELL_HEIGHT)?;
    let mut rgb = vec![0; width.checked_mul(height)?.checked_mul(3)?];
    let font = screenshot_font();

    if font.is_none() && !(cfg.mode == RenderMode::Blocks && cfg.style.emits_color()) {
        return None;
    }

    for cell_y in 0..rows {
        for cell_x in 0..cols {
            if let Some(cell) = sample_cell(frame, &geometry, cell_x, cell_y, cfg, time) {
                let color = if cell.emit_color {
                    color::quantize_rgb(cfg.effective_depth(), cell.rgb.0, cell.rgb.1, cell.rgb.2)
                } else {
                    SCREENSHOT_DEFAULT_FG
                };
                paint_screenshot_cell(&mut rgb, width, cell_x, cell_y, color, &cell, cfg, font);
            }
        }
    }

    Some(ScreenshotImage {
        width: width as u32,
        height: height as u32,
        rgb,
    })
}

fn paint_screenshot_cell(
    image: &mut [u8],
    image_width: usize,
    cell_x: u16,
    cell_y: u16,
    color: (u8, u8, u8),
    cell: &RenderedCell,
    cfg: &RenderConfig,
    font: Option<&FontArc>,
) {
    let x0 = usize::from(cell_x) * SCREENSHOT_CELL_WIDTH;
    let y0 = usize::from(cell_y) * SCREENSHOT_CELL_HEIGHT;

    if cfg.mode == RenderMode::Blocks && cell.emit_color {
        fill_screenshot_cell(image, image_width, x0, y0, color);
        return;
    }

    let Some(font) = font else {
        return;
    };
    draw_glyph(image, image_width, x0, y0, color, cell.ch, font);
}

fn fill_screenshot_cell(
    image: &mut [u8],
    image_width: usize,
    x0: usize,
    y0: usize,
    color: (u8, u8, u8),
) {
    for py in 0..SCREENSHOT_CELL_HEIGHT {
        let row = (y0 + py) * image_width * 3;
        for px in 0..SCREENSHOT_CELL_WIDTH {
            let i = row + (x0 + px) * 3;
            image[i] = color.0;
            image[i + 1] = color.1;
            image[i + 2] = color.2;
        }
    }
}

fn draw_glyph(
    image: &mut [u8],
    image_width: usize,
    x0: usize,
    y0: usize,
    color: (u8, u8, u8),
    ch: char,
    font: &FontArc,
) {
    let scaled = font.as_scaled(SCREENSHOT_FONT_SCALE);
    let glyph_id = font.glyph_id(ch);
    let advance = scaled.h_advance(glyph_id);
    let baseline =
        ((SCREENSHOT_CELL_HEIGHT as f32 - scaled.height()) * 0.5 + scaled.ascent()).round();
    let x = x0 as f32 + ((SCREENSHOT_CELL_WIDTH as f32 - advance) * 0.5).floor();
    let y = y0 as f32 + baseline;

    draw_glyph_at(
        image,
        image_width,
        x0,
        y0,
        color,
        font,
        glyph_id.with_scale_and_position(SCREENSHOT_FONT_SCALE, point(x, y)),
    );
    draw_glyph_at(
        image,
        image_width,
        x0,
        y0,
        color,
        font,
        glyph_id.with_scale_and_position(SCREENSHOT_FONT_SCALE, point(x + 0.45, y)),
    );
}

fn draw_glyph_at(
    image: &mut [u8],
    image_width: usize,
    x0: usize,
    y0: usize,
    color: (u8, u8, u8),
    font: &FontArc,
    glyph: ab_glyph::Glyph,
) {
    let Some(outlined) = font.outline_glyph(glyph) else {
        return;
    };
    let bounds = outlined.px_bounds();
    outlined.draw(|gx, gy, coverage| {
        if coverage <= 0.0 {
            return;
        }

        let px = bounds.min.x as i32 + gx as i32;
        let py = bounds.min.y as i32 + gy as i32;
        let dx = px - x0 as i32;
        let dy = py - y0 as i32;
        if dx < 0
            || dy < 0
            || dx >= SCREENSHOT_CELL_WIDTH as i32
            || dy >= SCREENSHOT_CELL_HEIGHT as i32
        {
            return;
        }

        let i = py as usize * image_width * 3 + px as usize * 3;
        let terminal_coverage = coverage.powf(SCREENSHOT_GLYPH_GAMMA).min(1.0);
        blend_channel(&mut image[i], color.0, terminal_coverage);
        blend_channel(&mut image[i + 1], color.1, terminal_coverage);
        blend_channel(&mut image[i + 2], color.2, terminal_coverage);
    });
}

fn blend_channel(dst: &mut u8, src: u8, coverage: f32) {
    let coverage = coverage.clamp(0.0, 1.0);
    *dst = (*dst as f32 * (1.0 - coverage) + src as f32 * coverage).round() as u8;
}

fn screenshot_font() -> Option<&'static FontArc> {
    static FONT: OnceLock<Option<FontArc>> = OnceLock::new();
    FONT.get_or_init(load_screenshot_font).as_ref()
}

fn load_screenshot_font() -> Option<FontArc> {
    for path in screenshot_font_candidates() {
        let Ok(bytes) = fs::read(path) else {
            continue;
        };
        let Ok(font) = FontArc::try_from_vec(bytes) else {
            continue;
        };
        return Some(font);
    }

    None
}

fn screenshot_font_candidates() -> &'static [&'static str] {
    &[
        "/System/Library/Fonts/SFNSMono.ttf",
        "/System/Library/Fonts/Menlo.ttc",
        "/System/Library/Fonts/Courier.ttc",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/truetype/liberation2/LiberationMono-Regular.ttf",
        "C:\\Windows\\Fonts\\consola.ttf",
        "C:\\Windows\\Fonts\\cour.ttf",
    ]
}

fn sample_cell(
    frame: &Frame,
    geometry: &RenderGeometry,
    c: u16,
    r: u16,
    cfg: &RenderConfig,
    time: f32,
) -> Option<RenderedCell> {
    let y0 = (geometry.crop_y0 + r as f32 * geometry.cell_h) as usize;
    let y1 = (geometry.crop_y0 + (r + 1) as f32 * geometry.cell_h).min(geometry.fh as f32) as usize;
    let cx = if cfg.mirror { geometry.cols - 1 - c } else { c };
    let x0 = (geometry.crop_x0 + cx as f32 * geometry.cell_w) as usize;
    let x1 =
        (geometry.crop_x0 + (cx + 1) as f32 * geometry.cell_w).min(geometry.fw as f32) as usize;

    let (mut sr, mut sg, mut sb, mut n) = (0u64, 0u64, 0u64, 0u64);
    for y in y0..y1 {
        let row_off = y * geometry.fw * 3;
        for x in x0..x1 {
            let i = row_off + x * 3;
            sr += frame.rgb[i] as u64;
            sg += frame.rgb[i + 1] as u64;
            sb += frame.rgb[i + 2] as u64;
            n += 1;
        }
    }
    if n == 0 {
        return None;
    }

    let brightness_offset = (cfg.brightness * 255.0) as i32;
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

    let ctx = StyleCtx {
        time,
        x: c,
        y: r,
        cols: geometry.cols,
        rows: geometry.rows,
    };
    let rgb = style::transform(cfg.style, (avg_r, avg_g, avg_b), &ctx);
    let luma = (0.299 * rgb.0 as f32 + 0.587 * rgb.1 as f32 + 0.114 * rgb.2 as f32) / 255.0;
    let emit_color = cfg.style.emits_color();
    let ch = match cfg.mode {
        RenderMode::Ascii => ascii::luma_to_char(luma, cfg.contrast, emit_color),
        // Color styles: color escapes carry brightness, so always solid █.
        // B&W: no color, fall back to shade ramp so the image is legible.
        RenderMode::Blocks if emit_color => '█',
        RenderMode::Blocks => ascii::luma_to_shade(luma, cfg.contrast),
    };

    Some(RenderedCell {
        rgb,
        ch,
        emit_color,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(rgb: (u8, u8, u8)) -> Frame {
        let mut pixels = Vec::with_capacity(12);
        for _ in 0..4 {
            pixels.extend_from_slice(&[rgb.0, rgb.1, rgb.2]);
        }

        Frame {
            width: 2,
            height: 2,
            rgb: pixels,
        }
    }

    #[test]
    fn screenshot_uses_cell_scale() {
        let cfg = RenderConfig::default();
        let image = render_screenshot(&frame((128, 128, 128)), 3, 2, &cfg, 0.0).unwrap();

        assert_eq!(image.width, 24);
        assert_eq!(image.height, 32);
        assert_eq!(image.rgb.len(), 24 * 32 * 3);
    }

    #[test]
    fn ascii_screenshot_renders_glyph_shape() {
        let mut cfg = RenderConfig::default();
        cfg.mode = RenderMode::Ascii;
        let ascii = render_screenshot(&frame((128, 128, 128)), 1, 1, &cfg, 0.0).unwrap();

        cfg.mode = RenderMode::Blocks;
        let blocks = render_screenshot(&frame((128, 128, 128)), 1, 1, &cfg, 0.0).unwrap();

        assert!(ascii.rgb.chunks_exact(3).any(|pixel| pixel == [0, 0, 0]));
        assert!(blocks.rgb.chunks_exact(3).all(|pixel| pixel != [0, 0, 0]));
    }
}
