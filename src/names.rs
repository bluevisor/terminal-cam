//! Forgiving name lookup shared by the config file and CLI flags.

/// Lowercase alphanumerics only, so `"Van Gogh"`, `van-gogh` and `VANGOGH`
/// compare equal.
fn key(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Returns the first item whose any of `names(item)` matches `input`.
pub fn find<T: Copy, const N: usize>(
    items: &[T],
    input: &str,
    names: impl Fn(&T) -> [&'static str; N],
) -> Option<T> {
    let wanted = key(input);
    if wanted.is_empty() {
        return None;
    }
    items
        .iter()
        .find(|item| names(item).iter().any(|n| key(n) == wanted))
        .copied()
}

#[cfg(test)]
mod tests {
    use crate::{color::ColorDepth, render::RenderMode, style::Style};

    #[test]
    fn parses_style_names_loosely() {
        assert_eq!(Style::from_name("van-gogh"), Some(Style::VanGogh));
        assert_eq!(Style::from_name("Van Gogh"), Some(Style::VanGogh));
        assert_eq!(Style::from_name("B&W"), Some(Style::BlackWhite));
        assert_eq!(Style::from_name("gray 5-step"), Some(Style::GrayFive));
        assert_eq!(Style::from_name("nope"), None);
        assert_eq!(Style::from_name(""), None);
    }

    #[test]
    fn every_style_round_trips_through_its_slug() {
        for style in crate::style::ALL {
            assert_eq!(Style::from_name(style.slug()), Some(style));
        }
    }

    #[test]
    fn parses_mode_and_depth() {
        assert_eq!(RenderMode::from_name("blocks"), Some(RenderMode::Blocks));
        assert_eq!(ColorDepth::from_name("256"), Some(ColorDepth::Palette256));
        assert_eq!(
            ColorDepth::from_name("TrueColor"),
            Some(ColorDepth::Truecolor)
        );
    }
}
