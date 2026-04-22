//! Color style transformations.
//!
//! Two families of transformations share one enum:
//!
//! 1. **Palette styles** (Sepia, VanGogh, Monet, Pixar, Simpsons) — throw
//!    away the source hue and look each pixel up in a 5-stop luminance
//!    gradient. This is why they look "flat" — it's the same trick art
//!    directors use when reducing reference photos to a limited palette.
//!
//! 2. **Trippy styles** (Mushroom, Lsd) — preserve luminance but rotate hue
//!    in HSV space over time and cell position. Converting to HSV for a
//!    single rotation and back is cheaper than doing full LUTs.
//!
//! `Color` and `BlackWhite` are passthrough — the only difference is whether
//! the renderer emits ANSI color escapes at all.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    Color,
    BlackWhite,
    Sepia,
    VanGogh,
    Monet,
    Pixar,
    Simpsons,
    Mushroom,
    Lsd,
}

pub const ALL: [Style; 9] = [
    Style::Color,
    Style::BlackWhite,
    Style::Sepia,
    Style::VanGogh,
    Style::Monet,
    Style::Pixar,
    Style::Simpsons,
    Style::Mushroom,
    Style::Lsd,
];

impl Style {
    pub fn label(self) -> &'static str {
        match self {
            Style::Color => "Full Color",
            Style::BlackWhite => "B&W",
            Style::Sepia => "Sepia",
            Style::VanGogh => "Van Gogh",
            Style::Monet => "Monet",
            Style::Pixar => "Pixar",
            Style::Simpsons => "Simpsons",
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
}

pub fn transform(style: Style, rgb: (u8, u8, u8), ctx: &StyleCtx) -> (u8, u8, u8) {
    let (r, g, b) = rgb;
    match style {
        Style::Color | Style::BlackWhite => (r, g, b),
        Style::Sepia => sepia(r, g, b),
        Style::VanGogh => gradient_map(r, g, b, &VAN_GOGH),
        Style::Monet => gradient_map(r, g, b, &MONET),
        Style::Pixar => gradient_map(r, g, b, &PIXAR),
        Style::Simpsons => gradient_map(r, g, b, &SIMPSONS),
        Style::Mushroom => trippy(r, g, b, ctx, 40.0, 0.3, 1.2),
        Style::Lsd => trippy(r, g, b, ctx, 220.0, 1.0, 0.7),
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

// ─── Luminance-gradient palette mapping ─────────────────────────────────────
type Stop = (f32, [u8; 3]);

static VAN_GOGH: [Stop; 5] = [
    (0.0, [15, 27, 66]),      // Starry Night deep navy
    (0.25, [41, 55, 110]),    // indigo
    (0.5, [95, 84, 71]),      // earth brown
    (0.75, [217, 178, 103]),  // gold
    (1.0, [246, 228, 166]),   // cream
];

static MONET: [Stop; 5] = [
    (0.0, [70, 77, 99]),      // slate
    (0.25, [120, 138, 161]),  // dusty blue
    (0.5, [178, 196, 188]),   // sage
    (0.75, [220, 196, 205]),  // pastel pink
    (1.0, [243, 228, 215]),   // cream
];

static PIXAR: [Stop; 5] = [
    (0.0, [30, 25, 55]),      // deep purple
    (0.25, [180, 90, 100]),   // coral
    (0.5, [240, 165, 80]),    // warm orange
    (0.75, [255, 215, 130]),  // warm yellow
    (1.0, [255, 250, 220]),   // cream
];

static SIMPSONS: [Stop; 5] = [
    (0.0, [40, 30, 120]),     // Marge blue
    (0.25, [210, 60, 50]),    // red
    (0.5, [255, 220, 60]),    // Simpson skin yellow
    (0.75, [255, 240, 100]),  // bright yellow
    (1.0, [255, 255, 200]),   // near white
];

fn gradient_map(r: u8, g: u8, b: u8, stops: &[Stop]) -> (u8, u8, u8) {
    let luma = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0;
    for pair in stops.windows(2) {
        let (l0, c0) = pair[0];
        let (l1, c1) = pair[1];
        if luma <= l1 {
            let t = ((luma - l0) / (l1 - l0)).clamp(0.0, 1.0);
            return lerp_rgb(c0, c1, t);
        }
    }
    let last = stops.last().unwrap().1;
    (last[0], last[1], last[2])
}

fn lerp_rgb(a: [u8; 3], b: [u8; 3], t: f32) -> (u8, u8, u8) {
    (
        (a[0] as f32 + t * (b[0] as f32 - a[0] as f32)) as u8,
        (a[1] as f32 + t * (b[1] as f32 - a[1] as f32)) as u8,
        (a[2] as f32 + t * (b[2] as f32 - a[2] as f32)) as u8,
    )
}

// ─── Trippy: rotate hue over time + position, boost saturation ──────────────
fn trippy(
    r: u8,
    g: u8,
    b: u8,
    ctx: &StyleCtx,
    hue_per_sec: f32,
    sat_boost: f32,
    val_gain: f32,
) -> (u8, u8, u8) {
    let (mut h, mut s, mut v) = rgb_to_hsv(r, g, b);
    h = (h + hue_per_sec * ctx.time + ctx.x as f32 * 3.0 + ctx.y as f32 * 2.0).rem_euclid(360.0);
    s = (s + sat_boost).clamp(0.0, 1.0);
    v = (v * val_gain).clamp(0.0, 1.0);
    hsv_to_rgb(h, s, v)
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
    fn gradient_endpoints() {
        assert_eq!(gradient_map(0, 0, 0, &VAN_GOGH), (15, 27, 66));
        assert_eq!(gradient_map(255, 255, 255, &VAN_GOGH), (246, 228, 166));
    }
}
