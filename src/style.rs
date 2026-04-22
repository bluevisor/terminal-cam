//! Color style transformations.
//!
//! Each style is a pure `(rgb, StyleCtx) -> rgb` function. The renderer
//! calls it per cell per frame, so everything here needs to be cheap.
//!
//! - **Sepia** — single affine RGB matrix.
//! - **Van Gogh** — static palette snap. Source hue picks one of three
//!   Van Gogh color ramps (cool / warm / green), source luma picks the
//!   anchor within that ramp. No animation — the painting doesn't move.
//! - **Monet** — pastelized HSV with a slow dappled-light mottle and
//!   atmospheric warm/cool hue shift.
//! - **Mushroom** — radial mandala (concentric rings + angular petals)
//!   plus a Julia-set iteration field whose sampling coordinates are
//!   domain-warped by source luma, so the fractal bends around objects.
//! - **LSD** — Julia-set iteration count drives hue rotation; both the
//!   sampling coordinates *and* the c-parameter are modulated by source
//!   luma, so different parts of the video render different fractal shapes.
//!
//! `Color` and `BlackWhite` are passthrough — the only difference is
//! whether the renderer emits ANSI color escapes at all.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    Color,
    BlackWhite,
    Sepia,
    VanGogh,
    Monet,
    Mushroom,
    Lsd,
}

pub const ALL: [Style; 7] = [
    Style::Color,
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
}

pub fn transform(style: Style, rgb: (u8, u8, u8), ctx: &StyleCtx) -> (u8, u8, u8) {
    let (r, g, b) = rgb;
    match style {
        Style::Color | Style::BlackWhite => (r, g, b),
        Style::Sepia => sepia(r, g, b),
        Style::VanGogh => van_gogh(r, g, b),
        Style::Monet => monet(r, g, b, ctx),
        Style::Mushroom => mushroom(r, g, b, ctx),
        Style::Lsd => lsd(r, g, b, ctx),
    }
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

// ─── Mushroom: mandala + slow-drifting Julia-set overlay ────────────────────
fn mushroom(r: u8, g: u8, b: u8, ctx: &StyleCtx) -> (u8, u8, u8) {
    let (mut h, mut s, mut v) = rgb_to_hsv(r, g, b);
    let cx = ctx.cols as f32 * 0.5;
    let cy = ctx.rows as f32 * 0.5;
    // Terminal cells are ~2:1 tall; pre-stretch y so rings look round.
    let dx = ctx.x as f32 - cx;
    let dy = (ctx.y as f32 - cy) * 2.0;
    let radius = (dx * dx + dy * dy).sqrt();
    let angle = dy.atan2(dx);
    let t = ctx.time;

    // Mandala: slowed by ~3× from the previous version.
    let ring = ((radius * 0.3 - t * 0.85).sin() * 0.5 + 0.5).clamp(0.0, 1.0);
    let petals = ((angle * 6.0 + t * 0.28).sin() * 0.5 + 0.5).clamp(0.0, 1.0);

    // Julia-set iteration field with image-driven domain warp: bright source
    // pixels displace the Julia sampling toward +x/+y, dark pixels toward
    // -x/-y, so the fractal's iso-lines curl around objects (face, edges)
    // instead of being locked to screen coordinates. c drifts slowly so the
    // underlying fractal still morphs.
    let luma_warp = luma(r, g, b) - 0.5; // -0.5..0.5
    let jx = (ctx.x as f32 - cx) / cx * 1.3 + luma_warp * 0.45;
    let jy = (ctx.y as f32 - cy) / cy * 0.65 + luma_warp * 0.25;
    let jcx = -0.40 + (t * 0.07).sin() * 0.15;
    let jcy = 0.60 + (t * 0.05).cos() * 0.15;
    let jc = julia_escape(jx, jy, jcx, jcy);

    h = (h + 30.0 + ring * 90.0 + petals * 55.0 + jc * 140.0 + t * 8.0).rem_euclid(360.0);
    s = (s + 0.20 + 0.22 * ring + 0.10 * jc).clamp(0.0, 1.0);
    v = (v * (0.68 + 0.32 * (ring * 0.4 + petals * 0.25 + jc * 0.35))).clamp(0.0, 1.0);

    hsv_to_rgb(h, s, v)
}

// ─── LSD: Julia-set-driven hue storm ────────────────────────────────────────
fn lsd(r: u8, g: u8, b: u8, ctx: &StyleCtx) -> (u8, u8, u8) {
    // Source saturation is discarded — LSD pins it near max.
    let (h0, _, v0) = rgb_to_hsv(r, g, b);
    let cx = ctx.cols as f32 * 0.5;
    let cy = ctx.rows as f32 * 0.5;
    let t = ctx.time;

    // Julia iteration count field with image-driven domain warp + small c
    // modulation. Domain warp bends Julia coordinates by source luma so
    // iso-lines curl around objects. c modulation means bright/dark regions
    // render slightly different Julia shapes — the fractal "is" the object
    // rather than sitting on top of it. Time drift keeps the overall shape
    // morphing through the Mandelbrot edge region.
    let luma_warp = luma(r, g, b) - 0.5;
    let jx = (ctx.x as f32 - cx) / cx * 1.5 + luma_warp * 0.5;
    let jy = (ctx.y as f32 - cy) / cy * 0.75 + luma_warp * 0.3;
    let jcx = -0.70 + (t * 0.06).sin() * 0.26 + luma_warp * 0.12;
    let jcy = 0.27 + (t * 0.045).cos() * 0.26 + luma_warp * 0.08;
    let jc = julia_escape(jx, jy, jcx, jcy);

    // Gentle secondary drift adds soft color motion inside the set.
    let (x, y) = (ctx.x as f32, ctx.y as f32);
    let drift = ((x * 0.07 + y * 0.05 + t * 0.10).sin()
        + (x * -0.04 + y * 0.08 - t * 0.08).sin())
        * 0.5;

    // jc*540 → the fractal boundary shows as visible hue bands (crossing the
    // escape-count gradient sweeps through the whole hue wheel). Hue drift is
    // slow (~18°/s) so the overall palette breathes instead of strobing.
    let h = (h0 + t * 18.0 + jc * 540.0 + drift * 50.0).rem_euclid(360.0);
    let s = (0.88 + drift * 0.10).clamp(0.0, 1.0);
    let v = (v0 * (0.5 + 0.5 * jc) + 0.05).clamp(0.0, 1.0);

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
        StyleCtx { time: 1.25, x: 12, y: 7, cols: 80, rows: 24 }
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
