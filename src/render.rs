//! Frame → terminal ASCII renderer.
//!
//! Per-cell pipeline:
//!   1. Average RGB over a source block.
//!   2. Apply user brightness offset.
//!   3. Apply style transform (Van Gogh, Alice, etc.).
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
use std::{fmt::Write as FmtWrite, fs, io::Write, sync::OnceLock};

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

/// Per-frame state that survives between renders. Currently used only by
/// Lucy's afterimage feedback buffer — the live render loop owns one of these
/// and passes a mutable reference into `render`. Reset whenever the terminal
/// resizes or the active style changes, so trails never leak across modes.
pub struct RenderState {
    trails: Vec<(u8, u8, u8)>,
    cols: u16,
    rows: u16,
    last_style: Option<Style>,
}

impl RenderState {
    pub fn new() -> Self {
        Self {
            trails: Vec::new(),
            cols: 0,
            rows: 0,
            last_style: None,
        }
    }

    fn prepare(&mut self, cols: u16, rows: u16, style: Style) {
        let needed = (cols as usize).saturating_mul(rows as usize);
        let resized = self.cols != cols || self.rows != rows;
        let style_changed = self.last_style != Some(style);
        if resized || style_changed || self.trails.len() != needed {
            self.trails.clear();
            self.trails.resize(needed, (0, 0, 0));
            self.cols = cols;
            self.rows = rows;
        }
        self.last_style = Some(style);
    }
}

impl Default for RenderState {
    fn default() -> Self {
        Self::new()
    }
}

/// Approximate terminal cell height-to-width ratio. Most fonts land near 2.0;
/// tweak here if your terminal renders tall (more like 2.2) or square (1.6).
const CHAR_ASPECT: f32 = 2.0;

/// Lucy afterimage retention. 0.85 means each frame keeps 85% of the previous
/// cell color and adds 15% of the fresh value — half-life ≈ 4.3 frames at
/// 30 FPS (~140 ms). Static scenes converge to the steady-state color so they
/// stay sharp; motion smears across the decay window.
const LUCY_TRAIL_DECAY: f32 = 0.85;
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
    state: &mut RenderState,
    mask: Option<(u16, u16, u16, u16)>,
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
    state.prepare(cols, rows, cfg.style);
    let depth = cfg.effective_depth();
    let mut last_fg: Option<Fg> = None;

    for r in 0..rows {
        // Skip camera cells under the menu — terminals without DEC 2026
        // sync would otherwise paint camera content first then overdraw
        // with the menu, which the user sees as flicker.
        let row_mask = mask.and_then(|(mx, my, mw, mh)| {
            (r >= my && r < my.saturating_add(mh))
                .then(|| (mx, mx.saturating_add(mw).min(cols)))
        });

        let mut c: u16 = 0;
        while c < cols {
            if let Some((mx0, mx1)) = row_mask {
                if c == mx0 {
                    // Decay the Lucy trail buffer under the masked rectangle so
                    // those cells fade naturally while the menu is open. Without
                    // this, masked cells keep their pre-menu trail values, and a
                    // ghost rectangle pops back when the menu closes.
                    if cfg.style == Style::Lucy {
                        for skip_c in mx0..mx1 {
                            let idx = (r as usize) * (cols as usize) + (skip_c as usize);
                            state.trails[idx] =
                                blend_rgb(state.trails[idx], (0, 0, 0), LUCY_TRAIL_DECAY);
                        }
                    }
                    let _ = write!(out, "\x1b[{}C", mx1 - mx0);
                    c = mx1;
                    continue;
                }
            }

            let Some(mut cell) = sample_cell(frame, &geometry, c, r, cfg, time) else {
                out.push(' ');
                c += 1;
                continue;
            };

            if cfg.style == Style::Lucy {
                apply_lucy_trail(&mut cell, state, cols, c, r, cfg);
            }

            if cell.emit_color {
                let (er, eg, eb) = if cfg.mode == RenderMode::Ascii {
                    ascii_brightness_compensation(cell.rgb)
                } else {
                    cell.rgb
                };
                let fg = color::quantize(depth, er, eg, eb);
                if last_fg != Some(fg) {
                    fg.write(out);
                    last_fg = Some(fg);
                }
            }
            out.push(cell.ch);
            c += 1;
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
    let (warp_dx_px, warp_dy_px) = if cfg.style == Style::Alice {
        alice_warp_pixels(geometry, c, r, time, cfg.mirror)
    } else {
        (0.0, 0.0)
    };

    let cx = if cfg.mirror { geometry.cols - 1 - c } else { c };

    // Alice samples each channel at a different offset for radial
    // chromatic aberration; everything else uses one shared block average.
    let (raw_r, raw_g, raw_b) = if cfg.style == Style::Alice {
        let (ca_dx, ca_dy) = alice_ca_offset_pixels(geometry, c, r, time, cfg.mirror);
        let r_avg =
            sample_channel_avg(frame, geometry, cx, r, warp_dx_px + ca_dx, warp_dy_px + ca_dy, 0);
        let g_avg = sample_channel_avg(frame, geometry, cx, r, warp_dx_px, warp_dy_px, 1);
        let b_avg =
            sample_channel_avg(frame, geometry, cx, r, warp_dx_px - ca_dx, warp_dy_px - ca_dy, 2);
        (r_avg, g_avg, b_avg)
    } else {
        let raw_y0 = geometry.crop_y0 + r as f32 * geometry.cell_h + warp_dy_px;
        let raw_y1 = geometry.crop_y0 + (r + 1) as f32 * geometry.cell_h + warp_dy_px;
        let raw_x0 = geometry.crop_x0 + cx as f32 * geometry.cell_w + warp_dx_px;
        let raw_x1 = geometry.crop_x0 + (cx + 1) as f32 * geometry.cell_w + warp_dx_px;
        let y0 = raw_y0.clamp(0.0, geometry.fh as f32) as usize;
        let y1 = raw_y1.clamp(0.0, geometry.fh as f32) as usize;
        let x0 = raw_x0.clamp(0.0, geometry.fw as f32) as usize;
        let x1 = raw_x1.clamp(0.0, geometry.fw as f32) as usize;

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
        ((sr / n) as u8, (sg / n) as u8, (sb / n) as u8)
    };

    let brightness_offset = (cfg.brightness * 255.0) as i32;
    let avg_r = contrast_u8(
        clamp_u8(raw_r as i32 + brightness_offset),
        cfg.contrast,
    );
    let avg_g = contrast_u8(
        clamp_u8(raw_g as i32 + brightness_offset),
        cfg.contrast,
    );
    let avg_b = contrast_u8(
        clamp_u8(raw_b as i32 + brightness_offset),
        cfg.contrast,
    );

    let (edge_x, edge_y, edge) =
        sample_edge(frame, geometry, c, r, cfg.mirror, warp_dx_px, warp_dy_px);
    let ctx = StyleCtx {
        time,
        x: c,
        y: r,
        cols: geometry.cols,
        rows: geometry.rows,
        edge_x,
        edge_y,
        edge,
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

/// Alice radial chromatic aberration: per-channel sampling offset that
/// grows from the screen center outward, sub-linear in radius so even
/// mid-frame cells get a clear fringe. R is sampled along +radial, B along
/// -radial, G unshifted — classic lens-CA polarity.
///
/// Magnitude breathes: a ~7 s sine cycles the corner peak between
/// ~0.8 cells (mild fringe) and ~8.3 cells (heavy splay). The motion is
/// what the user perceives as the image "breathing" in and out of focus.
fn alice_ca_offset_pixels(
    geometry: &RenderGeometry,
    c: u16,
    r: u16,
    time: f32,
    mirror: bool,
) -> (f32, f32) {
    let cx_center = geometry.cols as f32 * 0.5;
    let cy_center = geometry.rows as f32 * 0.5;
    let dx = (c as f32 + 0.5) - cx_center;
    let dy = (r as f32 + 0.5) - cy_center;
    let r_dist_sq = dx * dx + dy * dy;
    if r_dist_sq < 1.0 {
        return (0.0, 0.0);
    }
    let r_dist = r_dist_sq.sqrt();
    let r_max = (cx_center * cx_center + cy_center * cy_center).sqrt().max(1.0);

    // Breath: 0..1 sine at ~0.85 rad/s → period ≈ 7.4 s. Lifts the peak
    // amplitude from a calm baseline to a heavy splay and back.
    let breath = (time * 0.85).sin() * 0.5 + 0.5;
    let peak_cells = 0.8 + 7.5 * breath;
    // sqrt-shaped radial falloff: cells halfway out already get ~70% of peak,
    // so the fringes are visible across most of the frame, not just corners.
    let amount_cells = peak_cells * (r_dist / r_max).sqrt();
    let nx = dx / r_dist;
    let ny = dy / r_dist;

    let mut ox = nx * amount_cells * geometry.cell_w;
    if mirror {
        ox = -ox;
    }
    let oy = ny * amount_cells * geometry.cell_h;
    (ox, oy)
}

/// Block-average a single channel of the source frame over the cell's
/// (warp + per-channel) sampling window. The window is *edge-clamped*: when
/// the offset pushes its start past either edge of the frame, we slide the
/// whole window back so it samples the nearest in-bounds region at full
/// width. That produces a stretched border instead of a black fringe at
/// the screen edges (returning 0 here meant any channel pushed off-frame
/// dropped to zero independently, which the user saw as black leaking in).
fn sample_channel_avg(
    frame: &Frame,
    geometry: &RenderGeometry,
    cx_src: u16,
    r: u16,
    dx_px: f32,
    dy_px: f32,
    channel: usize,
) -> u8 {
    let raw_y0 = geometry.crop_y0 + r as f32 * geometry.cell_h + dy_px;
    let raw_x0 = geometry.crop_x0 + cx_src as f32 * geometry.cell_w + dx_px;
    let win_w = geometry.cell_w.max(1.0);
    let win_h = geometry.cell_h.max(1.0);

    let max_x_start = (geometry.fw as f32 - win_w).max(0.0);
    let max_y_start = (geometry.fh as f32 - win_h).max(0.0);
    let x0_f = raw_x0.clamp(0.0, max_x_start);
    let y0_f = raw_y0.clamp(0.0, max_y_start);
    let x0 = x0_f as usize;
    let y0 = y0_f as usize;
    let x1 = (x0_f + win_w).min(geometry.fw as f32) as usize;
    let y1 = (y0_f + win_h).min(geometry.fh as f32) as usize;

    let (mut s, mut n) = (0u64, 0u64);
    for y in y0..y1 {
        let row_off = y * geometry.fw * 3;
        for x in x0..x1 {
            s += frame.rgb[row_off + x * 3 + channel] as u64;
            n += 1;
        }
    }
    if n == 0 {
        0
    } else {
        (s / n) as u8
    }
}

/// Alice UV warp: shift each cell's source-frame sampling position by a
/// domain-warped two-octave sine field. The first octave is a coarse, slow
/// sine pair (dx depends on display-y, dy on display-x — so vertical edges
/// curl horizontally and horizontal edges wave vertically as the cursor
/// advances). The second octave is sampled in coordinates already displaced
/// by the first, which is the classic trick that produces fractal-layered
/// (Inigo-Quilez-style) noise on the cheap. Net amplitude ~3.7 cells so the
/// distortion is clearly visible while still preserving image content.
/// We flip dx under mirror so the curl direction matches what's drawn.
fn alice_warp_pixels(
    geometry: &RenderGeometry,
    c: u16,
    r: u16,
    time: f32,
    mirror: bool,
) -> (f32, f32) {
    let cols_f = geometry.cols.max(1) as f32;
    let rows_f = geometry.rows.max(1) as f32;
    let nx = (c as f32 + 0.5) / cols_f * 2.0 - 1.0;
    let ny = (r as f32 + 0.5) / rows_f * 2.0 - 1.0;

    let amp1 = 1.2;
    let dx1 = (ny * 2.2 + time * 0.55).sin() * amp1;
    let dy1 = (nx * 2.5 + time * 0.45).sin() * amp1;

    // Domain-warped second octave: feed the first warp's output back into
    // the input coordinates of a higher-frequency sine pair. The recursive
    // displacement breaks the regularity of pure sines and reads as fractal
    // bending rather than a smooth wave.
    let nx2 = nx + dx1 * 0.10;
    let ny2 = ny + dy1 * 0.10;
    let amp2 = 0.5;
    let dx2 = (ny2 * 5.5 - time * 0.70).sin() * amp2;
    let dy2 = (nx2 * 5.8 + time * 0.60).sin() * amp2;

    let dx_cells = dx1 + dx2;
    let dy_cells = dy1 + dy2;

    let mut dx_px = dx_cells * geometry.cell_w;
    if mirror {
        dx_px = -dx_px;
    }
    let dy_px = dy_cells * geometry.cell_h;
    (dx_px, dy_px)
}

/// Lucy afterimage: alpha-blend the freshly computed cell color against the
/// trail buffer, write the blend back into both the cell and the buffer, and
/// recompute the glyph from the blended luma so ASCII shading also fades
/// with the trail.
fn apply_lucy_trail(
    cell: &mut RenderedCell,
    state: &mut RenderState,
    cols: u16,
    c: u16,
    r: u16,
    cfg: &RenderConfig,
) {
    let idx = (r as usize) * (cols as usize) + (c as usize);
    let prev = state.trails[idx];
    let blended = blend_rgb(prev, cell.rgb, LUCY_TRAIL_DECAY);
    state.trails[idx] = blended;
    cell.rgb = blended;

    let luma = (0.299 * blended.0 as f32
        + 0.587 * blended.1 as f32
        + 0.114 * blended.2 as f32)
        / 255.0;
    cell.ch = match cfg.mode {
        RenderMode::Ascii => ascii::luma_to_char(luma, cfg.contrast, true),
        RenderMode::Blocks => '█',
    };
}

/// ASCII-mode color boost. ASCII glyphs only paint a fraction of each cell;
/// the rest shows terminal background, so a faithful color reads much darker
/// than the equivalent solid block. Multiply each channel by a luma-driven
/// factor (1.0 at white, ~1.85 at black) so dark/sparse cells emit a
/// strongly lifted painted stroke while bright cells barely change. Hue and
/// saturation are preserved because the multiplier is the same per channel.
fn ascii_brightness_compensation(rgb: (u8, u8, u8)) -> (u8, u8, u8) {
    let (r, g, b) = rgb;
    let luma = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0;
    let boost = 1.0 + (1.0 - luma).max(0.0) * 0.85;
    let lift = |c: u8| ((c as f32 * boost).min(255.0)) as u8;
    (lift(r), lift(g), lift(b))
}

fn blend_rgb(prev: (u8, u8, u8), new: (u8, u8, u8), decay: f32) -> (u8, u8, u8) {
    let mix = |p: u8, n: u8| -> u8 {
        (p as f32 * decay + n as f32 * (1.0 - decay))
            .round()
            .clamp(0.0, 255.0) as u8
    };
    (mix(prev.0, new.0), mix(prev.1, new.1), mix(prev.2, new.2))
}

/// Central-difference luma gradient at the cell's center pixel, with a
/// neighbour offset of one cell-width/-height. Returns `(gx, gy, magnitude)`
/// in normalized image-luma units (channels divided by 255). Magnitude is
/// clamped to [0, 1].
///
/// We mirror the X axis when the camera output is mirrored so the gradient
/// matches what's drawn on screen — otherwise edge-driven fractals would
/// warp opposite to the visible image.
fn sample_edge(
    frame: &Frame,
    geometry: &RenderGeometry,
    c: u16,
    r: u16,
    mirror: bool,
    warp_dx_px: f32,
    warp_dy_px: f32,
) -> (f32, f32, f32) {
    let cx = if mirror { geometry.cols - 1 - c } else { c };
    let center_x = geometry.crop_x0 + (cx as f32 + 0.5) * geometry.cell_w + warp_dx_px;
    let center_y = geometry.crop_y0 + (r as f32 + 0.5) * geometry.cell_h + warp_dy_px;
    let step_x = geometry.cell_w.max(1.0);
    let step_y = geometry.cell_h.max(1.0);

    let luma_at = |x: f32, y: f32| -> f32 {
        let xi = x.clamp(0.0, geometry.fw as f32 - 1.0) as usize;
        let yi = y.clamp(0.0, geometry.fh as f32 - 1.0) as usize;
        let i = (yi * geometry.fw + xi) * 3;
        let pr = frame.rgb[i] as f32;
        let pg = frame.rgb[i + 1] as f32;
        let pb = frame.rgb[i + 2] as f32;
        (0.299 * pr + 0.587 * pg + 0.114 * pb) / 255.0
    };

    let mut gx = luma_at(center_x + step_x, center_y) - luma_at(center_x - step_x, center_y);
    let gy = luma_at(center_x, center_y + step_y) - luma_at(center_x, center_y - step_y);
    if mirror {
        gx = -gx;
    }
    let mag = (gx * gx + gy * gy).sqrt().min(1.0);
    (gx, gy, mag)
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
