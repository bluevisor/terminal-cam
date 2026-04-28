//! Color style transformations.
//!
//! Each style is a pure `(rgb, StyleCtx) -> rgb` function. The renderer
//! calls it per cell per frame, so everything here needs to be cheap.
//!
//! - **Vivid** — saturation boost in HSV with a gentle gamma lift on value.
//! - **Sepia** — single affine RGB matrix.
//! - **Van Gogh** — static palette snap. Source hue picks one of three
//!   Van Gogh color ramps (cool / warm / green), source luma picks the
//!   anchor within that ramp. No animation — the painting doesn't move.
//! - **Monet** — pastelized HSV with a slow dappled-light mottle and
//!   atmospheric warm/cool hue shift.
//! - **Mushroom** — geometry-first style. Source colors pass through almost
//!   untouched; a Julia-set iteration field (edge-warped by the source
//!   gradient) carves dark fractal bands into the image via value
//!   modulation. The renderer also UV-warps the source sampling with a
//!   domain-warped two-octave sine field, so straight edges curl into
//!   moving fractal-layered curves.
//! - **LSD** — fluid-dynamic overlay. A scalar flow field is built from two
//!   octaves of domain-warped sine noise; the warp evolves with time, and
//!   the source-frame edge gradient pushes the flow *along* iso-lines (the
//!   tangent perpendicular to the gradient), so the fluid swirls along
//!   contours instead of straight across them. The flow value drives the
//!   same hue/sat/value formulas the previous LSD used, so the palette is
//!   unchanged.
//!
//! `Color` and `BlackWhite` are passthrough — the only difference is
//! whether the renderer emits ANSI color escapes at all.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    Color,
    Vivid,
    BlackWhite,
    Sepia,
    VanGogh,
    Monet,
    Mushroom,
    Lsd,
}

pub const ALL: [Style; 8] = [
    Style::Color,
    Style::Vivid,
    Style::BlackWhite,
    Style::Sepia,
    Style::VanGogh,
    Style::Monet,
    Style::Mushroom,
    Style::Lsd,
];

impl Style {
    pub fn label(self) -> &'static str {
        match self {
            Style::Color => "Color",
            Style::Vivid => "Vivid",
            Style::BlackWhite => "B&W",
            Style::Sepia => "Sepia",
            Style::VanGogh => "Van Gogh",
            Style::Monet => "Monet",
            Style::Mushroom => "Mushroom",
            Style::Lsd => "LSD",
        }
    }

    pub fn emits_color(self) -> bool {
        !matches!(self, Style::BlackWhite)
    }

    pub fn cycle(self, dir: i32) -> Self {
        let idx = ALL.iter().position(|&s| s == self).unwrap_or(0) as i32;
        let n = ALL.len() as i32;
        ALL[((idx + dir).rem_euclid(n)) as usize]
    }
}

pub struct StyleCtx {
    pub time: f32,
    pub x: u16,
    pub y: u16,
    pub cols: u16,
    pub rows: u16,
    /// Source-frame luma gradient at this cell, X component, ~[-1, 1].
    /// Sign points from dark to bright across the edge.
    pub edge_x: f32,
    /// Source-frame luma gradient at this cell, Y component, ~[-1, 1].
    pub edge_y: f32,
    /// Edge magnitude `sqrt(edge_x² + edge_y²)`, clamped to [0, 1].
    pub edge: f32,
}

pub fn transform(style: Style, rgb: (u8, u8, u8), ctx: &StyleCtx) -> (u8, u8, u8) {
    let (r, g, b) = rgb;
    match style {
        Style::Color | Style::BlackWhite => (r, g, b),
        Style::Vivid => vivid(r, g, b),
        Style::Sepia => sepia(r, g, b),
        Style::VanGogh => van_gogh(r, g, b),
        Style::Monet => monet(r, g, b, ctx),
        Style::Mushroom => mushroom(r, g, b, ctx),
        Style::Lsd => lsd(r, g, b, ctx),
    }
}

// ─── Vivid: saturation boost + mild gamma lift ──────────────────────────────
fn vivid(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let (h, s, v) = rgb_to_hsv(r, g, b);
    let s = (s * 1.55 + 0.04).clamp(0.0, 1.0);
    let v = v.powf(0.85);
    hsv_to_rgb(h, s, v)
}

// ─── Sepia ──────────────────────────────────────────────────────────────────
fn sepia(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let rf = r as f32;
    let gf = g as f32;
    let bf = b as f32;
    let nr = (0.393 * rf + 0.769 * gf + 0.189 * bf).min(255.0) as u8;
    let ng = (0.349 * rf + 0.686 * gf + 0.168 * bf).min(255.0) as u8;
    let nb = (0.272 * rf + 0.534 * gf + 0.131 * bf).min(255.0) as u8;
    (nr, ng, nb)
}

// ─── Van Gogh: static palette snap ──────────────────────────────────────────
//
// Each ramp is a discrete sequence of anchor colors hand-picked from the
// Irises / Starry Night / self-portrait palette. Source hue picks the ramp,
// source luma picks the anchor within it. The output has at most
// 3 ramps × 5 anchors = 15 distinct colors — which is roughly what a
// painter's limited palette produces.
const VG_COOL: [(u8, u8, u8); 5] = [
    (12, 18, 54),    // ink-dark ultramarine (deep shadow)
    (28, 52, 130),   // cobalt
    (75, 125, 200),  // mid blue
    (155, 195, 230), // pale verdigris
    (220, 235, 240), // blue-white highlight (moon halo)
];

const VG_WARM: [(u8, u8, u8); 5] = [
    (55, 30, 20),    // deep rust shadow
    (140, 70, 35),   // terracotta
    (210, 125, 40),  // warm orange
    (235, 190, 60),  // saturated gold
    (250, 235, 175), // cream highlight
];

const VG_GREEN: [(u8, u8, u8); 5] = [
    (55, 75, 40),    // deep moss
    (100, 130, 70),  // moss
    (155, 180, 125), // sage
    (205, 220, 130), // yellow-green
    (240, 240, 190), // pale yellow-cream
];

fn van_gogh(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let luma = luma(r, g, b);
    let (hue, sat, _) = rgb_to_hsv(r, g, b);

    // Low-chroma pixels go to the cool ramp so neutral source colour reads
    // as the painting's overall blue-dominant cast. Warm/green/cool hue
    // categories pick their respective ramps; no interpolation across the
    // hue wheel means no muddy cyan/green midtones.
    let ramp: &[(u8, u8, u8); 5] = if sat < 0.14 {
        &VG_COOL
    } else if !(65.0..330.0).contains(&hue) {
        &VG_WARM
    } else if hue < 180.0 {
        &VG_GREEN
    } else {
        &VG_COOL
    };

    // Stretch luma so mid-exposure camera doesn't collapse into one band.
    let stretched = smoothstep(0.08, 0.92, luma);
    let idx = ((stretched * ramp.len() as f32) as usize).min(ramp.len() - 1);
    ramp[idx]
}

// ─── Monet: pastel dapple with atmospheric warm/cool shift ──────────────────
fn monet(r: u8, g: u8, b: u8, ctx: &StyleCtx) -> (u8, u8, u8) {
    let (mut h, s, v) = rgb_to_hsv(r, g, b);
    let (x, y, t) = (ctx.x as f32, ctx.y as f32, ctx.time);
    // Low-freq product-of-sines: slow "dappled light through leaves".
    let dapple = (x * 0.13 + y * 0.11 + t * 0.25).sin()
        * (x * 0.09 - y * 0.14 - t * 0.18).sin();

    // Pastelize: cap saturation, lift gamma so mids bloom.
    let sat = (s * 0.40 + 0.14).clamp(0.0, 1.0);
    let val_mid = v.powf(0.65);

    // Atmospheric: highlights → warm cream, shadows → cool violet;
    // dapple wobbles hue so neighbours don't read as flat.
    let hue_shift = (val_mid - 0.5) * -48.0 + dapple * 12.0;
    h = (h + hue_shift).rem_euclid(360.0);

    let val = (val_mid * (0.88 + 0.12 * dapple.abs())).clamp(0.0, 1.0);
    hsv_to_rgb(h, sat, val)
}

// ─── Mushroom: edge-warped Julia field carving the source image ─────────────
fn mushroom(r: u8, g: u8, b: u8, ctx: &StyleCtx) -> (u8, u8, u8) {
    let (h, s, v) = rgb_to_hsv(r, g, b);
    let cx = ctx.cols as f32 * 0.5;
    let cy = ctx.rows as f32 * 0.5;
    let t = ctx.time;

    // Julia-set iteration field, sampled in screen-normalized coords with
    // a strong edge-driven domain warp: the source-frame luma gradient
    // pushes sampling far enough along its direction (×1.6 / ×0.95) that
    // the Julia iso-contours physically bend to align with image edges.
    // c drifts so the fractal still morphs over time.
    let jx = (ctx.x as f32 - cx) / cx * 1.3 + ctx.edge_x * 1.6;
    let jy = (ctx.y as f32 - cy) / cy * 0.65 + ctx.edge_y * 0.95;
    let jcx = -0.40 + (t * 0.07).sin() * 0.15;
    let jcy = 0.60 + (t * 0.05).cos() * 0.15;
    let jc = julia_escape(jx, jy, jcx, jcy); // 0 = escaped fast, 1 = inside set

    // Boundary band: 4·jc·(1−jc) peaks at jc=0.5 (the escape transition) and
    // tapers to 0 both inside and outside the set. Using it as the modulator
    // makes the fractal show as a thin dark/tinted ribbon along the
    // Julia boundary instead of a solid blob.
    let band = 4.0 * jc * (1.0 - jc);

    // Edge gate: the fractal pattern is only painted where the source has a
    // real edge. In flat regions (skin, walls) the gate is near zero so no
    // fractal carving appears; at moderate-to-strong contours (face outline,
    // hair, glasses) the gate saturates at 1, so the band rides those edges.
    // Combined with the edge-warped jx/jy above, this is what makes the
    // fractal *follow* edges instead of sitting in screen space.
    let edge_gate = (ctx.edge * 2.8).clamp(0.0, 1.0);
    let h_new = (h + band * 32.0 * edge_gate).rem_euclid(360.0);
    let v_new = (v * (1.0 - band * 0.6 * edge_gate)).clamp(0.0, 1.0);

    hsv_to_rgb(h_new, s, v_new)
}

// ─── LSD: edge-tangent fluid flow ───────────────────────────────────────────
fn lsd(r: u8, g: u8, b: u8, ctx: &StyleCtx) -> (u8, u8, u8) {
    // Source saturation is discarded — LSD pins it near max.
    let (h0, _, v0) = rgb_to_hsv(r, g, b);
    let t = ctx.time;

    // Normalize cell coords to roughly screen-space units (~[-1.5, 1.5] in x).
    // Y is doubled to compensate for terminal cells being ~2:1 tall, so swirls
    // look round rather than vertically squashed.
    let cx = ctx.cols as f32 * 0.5;
    let cy = ctx.rows as f32 * 0.5;
    let mut px = (ctx.x as f32 - cx) / cx * 1.5;
    let mut py = (ctx.y as f32 - cy) / cy * 0.75 * 2.0;

    // Curl-noise-ish domain warp: the warp at point p is built from sines of
    // p's *other* coordinate, which is the cheap trick that gives swirling,
    // divergence-free-looking flow rather than a grid pattern. Time evolution
    // is slow (≤0.4 rad/s) so the overall fluid breathes rather than strobes.
    let warp_x = ((py * 1.7 + t * 0.40).sin() + (py * 3.1 - t * 0.30).cos()) * 0.5;
    let warp_y = ((px * 1.7 - t * 0.35).sin() + (px * 3.1 + t * 0.32).cos()) * 0.5;

    // Edge tangent — perpendicular to the luma gradient — pushes the fluid
    // *along* iso-lines, so swirls stream around contours instead of crossing
    // them. (gx, gy) ⟂ (gy, -gx).
    let tangent_x = ctx.edge_y;
    let tangent_y = -ctx.edge_x;
    px += warp_x * 0.55 + tangent_x * 1.10;
    py += warp_y * 0.55 + tangent_y * 1.10;

    // Two octaves of warped sine noise, recombined as a unit-range scalar.
    // Octave weights (0.62 / 0.38) tilt toward the low frequency so the fluid
    // reads as smooth rivers with finer ripple, not white noise.
    let f1 = (px * 2.3 + t * 0.50).sin() * (py * 1.9 - t * 0.40).cos();
    let f2 = (px * 4.1 - t * 0.60).sin() * (py * 3.7 + t * 0.55).cos();
    let f = ((f1 * 0.62 + f2 * 0.38) * 0.5 + 0.5).clamp(0.0, 1.0);

    // Secondary drift in screen space adds soft, low-frequency color motion
    // that's independent of the warped fluid coords — same role as before.
    let (xs, ys) = (ctx.x as f32, ctx.y as f32);
    let drift = ((xs * 0.07 + ys * 0.05 + t * 0.10).sin()
        + (xs * -0.04 + ys * 0.08 - t * 0.08).sin())
        * 0.5;

    // Same palette mapping as the previous LSD: f * 540 sweeps the full hue
    // wheel along flow gradients, sat pinned near max, value gated by f so
    // dark/bright stripes carve through the field. Hue drift ~18°/s.
    let h = (h0 + t * 18.0 + f * 540.0 + drift * 50.0).rem_euclid(360.0);
    let s = (0.88 + drift * 0.10).clamp(0.0, 1.0);
    let v = (v0 * (0.5 + 0.5 * f) + 0.05).clamp(0.0, 1.0);

    hsv_to_rgb(h, s, v)
}

// ─── Julia set escape-time iteration ────────────────────────────────────────
/// Iterates z ← z² + c starting from (zx, zy). Returns the fraction of
/// allowed iterations before |z| escapes 2. Inside the set → 1.0.
fn julia_escape(zx: f32, zy: f32, cx: f32, cy: f32) -> f32 {
    const MAX_ITER: u32 = 18;
    let (mut x, mut y) = (zx, zy);
    for i in 0..MAX_ITER {
        let xn = x * x - y * y + cx;
        let yn = 2.0 * x * y + cy;
        if xn * xn + yn * yn > 4.0 {
            return i as f32 / MAX_ITER as f32;
        }
        x = xn;
        y = yn;
    }
    1.0
}

// ─── Helpers ────────────────────────────────────────────────────────────────
fn luma(r: u8, g: u8, b: u8) -> f32 {
    (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn rgb_to_hsv(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let r = r as f32 / 255.0;
    let g = g as f32 / 255.0;
    let b = b as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max == 0.0 { 0.0 } else { d / max };
    (h, s, max)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let h6 = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - ((h6 % 2.0) - 1.0).abs());
    let (r, g, b) = match h6 as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    (
        ((r + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((g + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((b + m) * 255.0).clamp(0.0, 255.0) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> StyleCtx {
        StyleCtx {
            time: 1.25,
            x: 12,
            y: 7,
            cols: 80,
            rows: 24,
            edge_x: 0.4,
            edge_y: -0.3,
            edge: 0.5,
        }
    }

    #[test]
    fn cycle_wraps() {
        let mut s = Style::Color;
        for _ in 0..ALL.len() {
            s = s.cycle(1);
        }
        assert_eq!(s, Style::Color);
    }

    #[test]
    fn hsv_roundtrip_approx() {
        for &(r, g, b) in &[(255u8, 0u8, 0u8), (128, 200, 64), (30, 40, 200)] {
            let (h, s, v) = rgb_to_hsv(r, g, b);
            let (nr, ng, nb) = hsv_to_rgb(h, s, v);
            assert!((nr as i32 - r as i32).abs() <= 1);
            assert!((ng as i32 - g as i32).abs() <= 1);
            assert!((nb as i32 - b as i32).abs() <= 1);
        }
    }

    #[test]
    fn styles_dont_panic_on_extremes() {
        let c = ctx();
        for &(r, g, b) in &[(0u8, 0u8, 0u8), (255, 255, 255), (128, 64, 200)] {
            for &style in &ALL {
                let _ = transform(style, (r, g, b), &c);
            }
        }
    }
}
