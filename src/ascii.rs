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

/// Apply contrast around 0.5, then shape with a sigmoid before indexing the ramp.
/// In color mode the truecolor fg carries brightness, so a steep curve (punchy)
/// is free. In B&W the glyph is the only signal — use a softer curve so the
/// middle of the ramp (where most gradations live) stays in play.
pub fn luma_to_char(luma: f32, contrast: f32, color: bool) -> char {
    let boosted = ((luma - 0.5) * contrast + 0.5).clamp(0.0, 1.0);
    let k = if color { 12.0 } else { 5.0 };
    let shaped = 1.0 / (1.0 + (-k * (boosted - 0.5)).exp());
    let idx = (shaped * (RAMP.len() - 1) as f32).round() as usize;
    RAMP[idx]
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
    fn bw_mode_preserves_midtone_headroom() {
        // Softer curve keeps endpoints off the ramp extremes so midtones
        // have characters to spare.
        let dark = luma_to_char(0.0, 1.0, false);
        let bright = luma_to_char(1.0, 1.0, false);
        let dark_idx = RAMP.iter().position(|&c| c == dark).unwrap();
        let bright_idx = RAMP.iter().position(|&c| c == bright).unwrap();
        assert!(dark_idx < 10, "dark should be near the sparse end, got {dark_idx}");
        assert!(
            bright_idx > RAMP.len() - 15 && bright_idx < RAMP.len() - 1,
            "bright should be dense but not maxed, got {bright_idx}"
        );
    }
}
