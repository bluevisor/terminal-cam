//! ASCII density ramp + luminance-to-glyph mapping.
//!
//! `RAMP` holds all 95 printable ASCII chars (0x20..=0x7E), sorted from
//! lightest (space) to densest (`$`) by approximate ink coverage in a standard
//! monospace font. The ordering is hand-tuned — close enough that the mapping
//! function's shape dominates the visual feel.

pub const RAMP: [char; 95] = [
    ' ', '`', '.', '\'', '-', ',', '_', ':', '^', '"', ';', '~', '!', '|', 'i',
    'l', '/', '\\', 'I', 'j', '(', ')', '[', ']', '{', '}', 't', '?', '+', '<',
    '>', '*', '=', 'r', 'c', 'v', 'x', 'y', 'z', 's', 'n', 'u', 'o', 'L', 'T',
    '7', '1', 'J', 'Y', 'C', 'F', 'Z', '2', '3', '4', '5', '6', 'w', 'a', 'e',
    'f', 'k', 'h', 'b', 'd', 'p', 'q', 'g', 'm', 'P', 'G', 'E', '9', '0', '8',
    'A', 'H', 'N', 'U', 'V', 'S', 'K', 'X', 'R', 'D', 'B', 'O', 'Q', 'M', 'W',
    '@', '#', '&', '%', '$',
];

/// Apply contrast around 0.5, then shape with a sigmoid and rescale so the
/// endpoints always land on ramp[0] (space) and ramp[last] (`$`). Without the
/// rescale the B&W k=5 sigmoid would map deep black to `:` (idx 7), leaving
/// a faint speckle in shadows — rescaling pins black to actual space.
/// Color mode uses a steep curve (k=12); B&W uses a softer curve (k=5) so
/// the middle of the ramp has room for midtone gradations, plus a gamma 1.5
/// pre-shape that pushes midtones toward darker glyphs (a flat sigmoid put
/// luma=0.5 on ramp[47]=`T`, which read as much brighter than a real Zone V
/// midtone would on a B&W photo).
pub fn luma_to_char(luma: f32, contrast: f32, color: bool) -> char {
    let boosted = ((luma - 0.5) * contrast + 0.5).clamp(0.0, 1.0);
    let toned = if color { boosted } else { boosted.powf(1.5) };
    let k = if color { 12.0 } else { 5.0 };
    let shaped = 1.0 / (1.0 + (-k * (toned - 0.5)).exp());
    let lo = 1.0 / (1.0 + (k * 0.5).exp()); // sigmoid at toned=0
    let hi = 1.0 / (1.0 + (-k * 0.5).exp()); // sigmoid at toned=1
    let normalized = ((shaped - lo) / (hi - lo)).clamp(0.0, 1.0);
    let idx = (normalized * (RAMP.len() - 1) as f32).round() as usize;
    RAMP[idx]
}

/// 5-stop Unicode shading ramp. Used for B&W + Blocks mode where there's
/// no color channel to carry brightness, so the glyph itself must. Same
/// gamma 1.5 tone curve as `luma_to_char` so a midtone luma shifts from
/// `▒` down to `░`, matching the perceived brightness of a B&W photo.
pub fn luma_to_shade(luma: f32, contrast: f32) -> char {
    const SHADES: [char; 5] = [' ', '░', '▒', '▓', '█'];
    let boosted = ((luma - 0.5) * contrast + 0.5).clamp(0.0, 1.0);
    let toned = boosted.powf(1.5);
    let idx = (toned * (SHADES.len() - 1) as f32).round() as usize;
    SHADES[idx]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramp_is_exactly_95_chars() {
        assert_eq!(RAMP.len(), 95);
    }

    #[test]
    fn color_mode_saturates_extremes() {
        assert_eq!(luma_to_char(0.0, 1.0, true), ' ');
        assert_eq!(luma_to_char(1.0, 1.0, true), '$');
    }

    #[test]
    fn bw_mode_saturates_extremes() {
        // Normalization pins endpoints to the ramp extremes — deep black
        // should render as space, peak white as '$'.
        assert_eq!(luma_to_char(0.0, 1.0, false), ' ');
        assert_eq!(luma_to_char(1.0, 1.0, false), '$');
    }

    #[test]
    fn bw_mode_preserves_midtone_headroom() {
        // The softer B&W sigmoid should still keep near-shadow / near-
        // highlight values inside the ramp interior, not collapse to the
        // extremes the way a steep color curve does.
        let mid_low = luma_to_char(0.25, 1.0, false);
        let mid_high = luma_to_char(0.75, 1.0, false);
        let mid_low_idx = RAMP.iter().position(|&c| c == mid_low).unwrap();
        let mid_high_idx = RAMP.iter().position(|&c| c == mid_high).unwrap();
        assert!(
            mid_low_idx > 5,
            "luma=0.25 shouldn't collapse near space, got idx {mid_low_idx}"
        );
        assert!(
            mid_high_idx < RAMP.len() - 6,
            "luma=0.75 shouldn't collapse near '$', got idx {mid_high_idx}"
        );
    }
}
