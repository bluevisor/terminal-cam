//! Centered options overlay.

use crate::{camera::CameraInfo, render::RenderConfig};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::fmt::Write as _;
use std::io::Write;

pub struct MenuState {
    pub selected: usize,
    pub cameras: Vec<CameraInfo>,
    pub current_camera: u32,
}

pub enum Action {
    None,
    Close,
    Quit,
    SwitchCamera(u32),
    CycleStyle(i32),
    ToggleMirror,
    AdjustBrightness(f32),
    AdjustContrast(f32),
}

const ITEMS: [&str; 7] = [
    "Camera source",
    "Style",
    "Mirror",
    "Brightness",
    "Contrast",
    "Close menu (Esc)",
    "Quit (q)",
];

impl MenuState {
    pub fn new(cameras: Vec<CameraInfo>, current_camera: u32) -> Self {
        Self { selected: 0, cameras, current_camera }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => Action::Close,
            (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => Action::Quit,
            (KeyCode::Up, _) => {
                self.selected = (self.selected + ITEMS.len() - 1) % ITEMS.len();
                Action::None
            }
            (KeyCode::Down, _) => {
                self.selected = (self.selected + 1) % ITEMS.len();
                Action::None
            }
            (KeyCode::Left, _) => self.adjust(-1),
            (KeyCode::Right, _) | (KeyCode::Enter, _) => self.adjust(1),
            _ => Action::None,
        }
    }

    fn adjust(&mut self, dir: i32) -> Action {
        match self.selected {
            0 => {
                if self.cameras.is_empty() {
                    return Action::None;
                }
                let cur = self
                    .cameras
                    .iter()
                    .position(|c| c.index == self.current_camera)
                    .unwrap_or(0) as i32;
                let n = self.cameras.len() as i32;
                let next = ((cur + dir).rem_euclid(n)) as usize;
                let idx = self.cameras[next].index;
                self.current_camera = idx;
                Action::SwitchCamera(idx)
            }
            1 => Action::CycleStyle(dir),
            2 => Action::ToggleMirror,
            3 => Action::AdjustBrightness(dir as f32 * 0.05),
            4 => Action::AdjustContrast(dir as f32 * 0.1),
            5 => Action::Close,
            6 => Action::Quit,
            _ => Action::None,
        }
    }
}

pub fn draw(state: &MenuState, cfg: &RenderConfig, cols: u16, rows: u16) -> std::io::Result<()> {
    let width: u16 = 54;
    let height: u16 = (ITEMS.len() as u16) + 6;
    let x0 = cols.saturating_sub(width) / 2;
    let y0 = rows.saturating_sub(height) / 2;

    let mut buf = String::with_capacity(4096);
    buf.push_str("\x1b[0m");

    // Border.
    let _ = write!(buf, "\x1b[{};{}H╭{}╮", y0 + 1, x0 + 1, "─".repeat(width as usize - 2));
    for row in 1..height - 1 {
        let _ = write!(
            buf,
            "\x1b[{};{}H│{}│",
            y0 + 1 + row,
            x0 + 1,
            " ".repeat(width as usize - 2)
        );
    }
    let _ = write!(
        buf,
        "\x1b[{};{}H╰{}╯",
        y0 + height,
        x0 + 1,
        "─".repeat(width as usize - 2)
    );

    // Title.
    let title = " terminal-cam · options ";
    let tx = x0 + (width - title.len() as u16) / 2;
    let _ = write!(buf, "\x1b[{};{}H\x1b[1m{}\x1b[0m", y0 + 2, tx + 1, title);

    // Items.
    for (i, item) in ITEMS.iter().enumerate() {
        let y = y0 + 4 + i as u16;
        let value = match i {
            0 => state
                .cameras
                .iter()
                .find(|c| c.index == state.current_camera)
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "(none)".into()),
            1 => cfg.style.label().to_string(),
            2 => if cfg.mirror { "on".into() } else { "off".into() },
            3 => format!("{:+.2}", cfg.brightness),
            4 => format!("{:.1}", cfg.contrast),
            _ => String::new(),
        };
        let selected = i == state.selected;
        let marker = if selected { "▸" } else { " " };
        let line = if value.is_empty() {
            format!(" {} {}", marker, item)
        } else {
            format!(" {} {:<18} {}", marker, item, value)
        };
        let interior = width as usize - 4;
        let trimmed: String = line.chars().take(interior).collect();
        let pad = interior.saturating_sub(trimmed.chars().count());
        let style = if selected { "\x1b[7m" } else { "" };
        let _ = write!(
            buf,
            "\x1b[{};{}H{}{}{}\x1b[0m",
            y + 1,
            x0 + 3,
            style,
            trimmed,
            " ".repeat(pad)
        );
    }

    // Hint.
    let hint = "↑↓ select · ←→ change · Enter apply · Esc close · q quit";
    let hint_len = hint.chars().count() as u16;
    let hx = x0 + width.saturating_sub(hint_len) / 2;
    let _ = write!(
        buf,
        "\x1b[{};{}H\x1b[2m{}\x1b[0m",
        y0 + height - 1,
        hx + 1,
        hint
    );

    let mut out = std::io::stdout().lock();
    out.write_all(buf.as_bytes())?;
    out.flush()
}
