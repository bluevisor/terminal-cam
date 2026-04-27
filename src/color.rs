//! Color depth resolution + RGB quantization.
//!
//! Three target depths:
//!
//! - **Truecolor** — `\x1b[38;2;R;G;Bm`, 16.7M colors. Modern terminals.
//! - **Palette256** — `\x1b[38;5;Nm`, 6×6×6 color cube + 24 grays + 16
//!   system. Supported by essentially every terminal that claims
//!   `xterm-256color`, including Apple Terminal.app.
//! - **Ansi16** — `\x1b[{30..37,90..97}m`, 16 fixed colors. Fallback for
//!   very old or constrained environments.
//!
//! Detection reads `COLORTERM` (set to `truecolor`/`24bit` by iTerm2, Warp,
//! Kitty, Alacritty, Ghostty, VS Code, tmux in passthrough mode). If that's
//! missing we fall back to 256-color, which renders banded but correct
//! everywhere.

use std::env;
use std::fmt::Write;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorDepth {
    Auto,
    Truecolor,
    Palette256,
    Ansi16,
}

pub const CYCLE: [ColorDepth; 4] = [
    ColorDepth::Auto,
    ColorDepth::Truecolor,
    ColorDepth::Palette256,
    ColorDepth::Ansi16,
];

impl ColorDepth {
    pub fn label(self) -> &'static str {
        match self {
            ColorDepth::Auto => "auto",
            ColorDepth::Truecolor => "truecolor",
            ColorDepth::Palette256 => "256",
            ColorDepth::Ansi16 => "16",
        }
    }

    pub fn cycle(self, dir: i32) -> Self {
        let idx = CYCLE.iter().position(|&d| d == self).unwrap_or(0) as i32;
        let n = CYCLE.len() as i32;
        CYCLE[((idx + dir).rem_euclid(n)) as usize]
    }

    pub fn resolve(self, detected: ColorDepth) -> ColorDepth {
        if self == ColorDepth::Auto {
            detected
        } else {
            self
        }
    }
}

pub fn detect() -> ColorDepth {
    if let Ok(v) = env::var("COLORTERM") {
        let v = v.to_ascii_lowercase();
        if v.contains("truecolor") || v.contains("24bit") {
            return ColorDepth::Truecolor;
        }
    }
    if let Ok(term) = env::var("TERM") {
        if term.contains("256") {
            return ColorDepth::Palette256;
        }
    }
    ColorDepth::Palette256
}

/// A foreground color selection that's canonical enough to dedup against.
/// Two cells with different source RGBs quantize to the same variant when
/// they'd emit the same escape — skip the redundant write in the renderer.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Fg {
    Rgb(u8, u8, u8),
    Indexed(u8),
    Ansi(u8),
}

impl Fg {
    pub fn write(self, out: &mut String) {
        match self {
            Fg::Rgb(r, g, b) => {
                let _ = write!(out, "\x1b[38;2;{};{};{}m", r, g, b);
            }
            Fg::Indexed(code) => {
                let _ = write!(out, "\x1b[38;5;{}m", code);
            }
            Fg::Ansi(code) => {
                let _ = write!(out, "\x1b[{}m", code);
            }
        }
    }
}

pub fn quantize(depth: ColorDepth, r: u8, g: u8, b: u8) -> Fg {
    match depth {
        ColorDepth::Truecolor => Fg::Rgb(r, g, b),
        ColorDepth::Palette256 => Fg::Indexed(rgb_to_256(r, g, b)),
        ColorDepth::Ansi16 => Fg::Ansi(rgb_to_16(r, g, b)),
        ColorDepth::Auto => {
            unreachable!("quantize requires a resolved depth; call ColorDepth::resolve first")
        }
    }
}

pub fn quantize_rgb(depth: ColorDepth, r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    match quantize(depth, r, g, b) {
        Fg::Rgb(r, g, b) => (r, g, b),
        Fg::Indexed(code) => indexed_to_rgb(code),
        Fg::Ansi(code) => ansi_code_to_rgb(code),
    }
}

// ─── 256-color: nearest of (6×6×6 cube) ∪ (24 grays) ────────────────────────
const CUBE_STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn cube_step(v: u8) -> u8 {
    match v {
        0..=47 => 0,
        48..=114 => 1,
        115..=154 => 2,
        155..=194 => 3,
        195..=234 => 4,
        _ => 5,
    }
}

fn rgb_to_256(r: u8, g: u8, b: u8) -> u8 {
    let cr = cube_step(r);
    let cg = cube_step(g);
    let cb = cube_step(b);
    let cube_idx = 16 + 36 * cr + 6 * cg + cb;
    let (cvr, cvg, cvb) = (CUBE_STEPS[cr as usize], CUBE_STEPS[cg as usize], CUBE_STEPS[cb as usize]);

    // Gray ramp: 24 values 8, 18, 28, …, 238, at indices 232..=255.
    let gray_avg = ((r as u32 + g as u32 + b as u32) / 3) as u8;
    let (gray_idx, gray_val) = if gray_avg < 8 {
        (16u8, 0u8)
    } else if gray_avg >= 238 {
        (231u8, 255u8)
    } else {
        let step = ((gray_avg - 8) / 10).min(23);
        (232 + step, 8 + step * 10)
    };

    if dist_sq(r, g, b, cvr, cvg, cvb) <= dist_sq(r, g, b, gray_val, gray_val, gray_val) {
        cube_idx
    } else {
        gray_idx
    }
}

fn indexed_to_rgb(code: u8) -> (u8, u8, u8) {
    match code {
        0..=15 => ANSI16[code as usize],
        16..=231 => {
            let idx = code - 16;
            let r = idx / 36;
            let g = (idx % 36) / 6;
            let b = idx % 6;
            (
                CUBE_STEPS[r as usize],
                CUBE_STEPS[g as usize],
                CUBE_STEPS[b as usize],
            )
        }
        232..=255 => {
            let value = 8 + (code - 232) * 10;
            (value, value, value)
        }
    }
}

// ─── 16-color ANSI: nearest fixed palette entry ─────────────────────────────
const ANSI16: [(u8, u8, u8); 16] = [
    (0, 0, 0),
    (128, 0, 0),
    (0, 128, 0),
    (128, 128, 0),
    (0, 0, 128),
    (128, 0, 128),
    (0, 128, 128),
    (192, 192, 192),
    (128, 128, 128),
    (255, 0, 0),
    (0, 255, 0),
    (255, 255, 0),
    (0, 0, 255),
    (255, 0, 255),
    (0, 255, 255),
    (255, 255, 255),
];

fn rgb_to_16(r: u8, g: u8, b: u8) -> u8 {
    let (mut best, mut best_d) = (0usize, u32::MAX);
    for (i, &(cr, cg, cb)) in ANSI16.iter().enumerate() {
        let d = dist_sq(r, g, b, cr, cg, cb);
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    if best < 8 {
        30 + best as u8
    } else {
        90 + (best - 8) as u8
    }
}

fn ansi_code_to_rgb(code: u8) -> (u8, u8, u8) {
    match code {
        30..=37 => ANSI16[(code - 30) as usize],
        90..=97 => ANSI16[(code - 90 + 8) as usize],
        _ => ANSI16[7],
    }
}

fn dist_sq(a0: u8, a1: u8, a2: u8, b0: u8, b1: u8, b2: u8) -> u32 {
    let d0 = a0 as i32 - b0 as i32;
    let d1 = a1 as i32 - b1 as i32;
    let d2 = a2 as i32 - b2 as i32;
    (d0 * d0 + d1 * d1 + d2 * d2) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_black_white_land_on_extremes() {
        assert_eq!(rgb_to_256(0, 0, 0), 16);
        assert_eq!(rgb_to_256(255, 255, 255), 231);
    }

    #[test]
    fn pure_colors_map_reasonably() {
        // Pure red should land on cube (5,0,0) = 16 + 180 = 196.
        assert_eq!(rgb_to_256(255, 0, 0), 196);
        // Pure green: cube (0,5,0) = 16 + 30 = 46.
        assert_eq!(rgb_to_256(0, 255, 0), 46);
        // Pure blue: cube (0,0,5) = 16 + 5 = 21.
        assert_eq!(rgb_to_256(0, 0, 255), 21);
    }

    #[test]
    fn mid_gray_prefers_ramp_over_cube() {
        // 128,128,128 is closer to gray-ramp index ~244 than cube (2,2,2)=139,139,139.
        let idx = rgb_to_256(128, 128, 128);
        assert!(idx >= 232, "expected gray ramp, got {idx}");
    }

    #[test]
    fn quantized_rgb_matches_palette_color() {
        assert_eq!(quantize_rgb(ColorDepth::Palette256, 255, 0, 0), (255, 0, 0));
        assert_eq!(
            quantize_rgb(ColorDepth::Ansi16, 255, 255, 255),
            (255, 255, 255)
        );
    }
}
