//! Centered options overlay.

use crate::{camera::CameraInfo, color::ColorDepth, render::RenderConfig};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::fmt::Write as _;
use std::path::PathBuf;

pub struct MenuState {
    pub selected: usize,
    pub cameras: Vec<CameraInfo>,
    pub current_camera: u32,
    pub screenshot_dir: PathBuf,
    path_editor: Option<String>,
}

pub enum Action {
    None,
    Close,
    Quit,
    SwitchCamera(u32),
    CycleStyle(i32),
    CycleMode(i32),
    CycleDepth(i32),
    ToggleMirror,
    AdjustBrightness(f32),
    AdjustContrast(f32),
    SetScreenshotDir(String),
}

const ITEMS: [&str; 10] = [
    "Camera source",
    "Style",
    "Render mode",
    "Color depth",
    "Mirror",
    "Brightness",
    "Contrast",
    "Screenshot path",
    "Close menu (Esc)",
    "Quit (q)",
];

// 3-row half-block title. Each letter is 1–5 cells wide × 3 rows tall, so
// 6 pixel rows of resolution — enough for E's middle bar (2-row layout
// only had 4 pixel rows, which collapsed E to look identical to C).
// Unicode half-block chars (▀ top, ▄ bottom, █ full) stack vertically.
const TITLE_LINE_1: &str = "▀█▀ █▀▀ █▀▄ █▄▀▄█ █ █▄ █ ▄▀█ █     ▄▀▀ ▄▀█ █▄▀▄█";
const TITLE_LINE_2: &str = " █  █▀▀ █▀▄ █ ▀ █ █ █ ▀█ █▀█ █     █   █▀█ █ ▀ █";
const TITLE_LINE_3: &str = " █  █▄▄ ▀▄▄ █   █ █ █  █ █ █ █▄▄ ▀▄▄ █ █ █   █";
const TITLE_COLS: u16 = 48;

impl MenuState {
    pub fn new(cameras: Vec<CameraInfo>, current_camera: u32, screenshot_dir: PathBuf) -> Self {
        Self {
            selected: 0,
            cameras,
            current_camera,
            screenshot_dir,
            path_editor: None,
        }
    }

    pub fn set_screenshot_dir(&mut self, screenshot_dir: PathBuf) {
        self.screenshot_dir = screenshot_dir;
    }

    pub fn is_editing_text(&self) -> bool {
        self.path_editor.is_some()
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        if self.path_editor.is_some() {
            return self.on_path_editor_key(key);
        }

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

    fn on_path_editor_key(&mut self, key: KeyEvent) -> Action {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.path_editor = None;
            return Action::Quit;
        }

        match key.code {
            KeyCode::Esc => {
                self.path_editor = None;
                Action::None
            }
            KeyCode::Enter => {
                let value = self.path_editor.take().unwrap_or_default();
                Action::SetScreenshotDir(value)
            }
            KeyCode::Backspace => {
                if let Some(editor) = &mut self.path_editor {
                    editor.pop();
                }
                Action::None
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                if let Some(editor) = &mut self.path_editor {
                    editor.push(c);
                }
                Action::None
            }
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
            2 => Action::CycleMode(dir),
            3 => Action::CycleDepth(dir),
            4 => Action::ToggleMirror,
            5 => Action::AdjustBrightness(dir as f32 * 0.05),
            6 => Action::AdjustContrast(dir as f32 * 0.1),
            7 => {
                self.path_editor = Some(self.screenshot_dir.display().to_string());
                Action::None
            }
            8 => Action::Close,
            9 => Action::Quit,
            _ => Action::None,
        }
    }
}

/// 0-indexed bounding box `(x0, y0, w, h)` of the centred overlay. Used by
/// the renderer to skip writing camera cells underneath the menu — without
/// that mask, terminals that don't support DEC 2026 synchronized updates
/// repaint the camera content before the menu overdraws each frame, which
/// the user sees as flicker. Width/height are clipped to the visible grid
/// so the mask never extends past `cols`/`rows`.
pub fn bounds(state: &MenuState, cols: u16, rows: u16) -> (u16, u16, u16, u16) {
    let (x0, y0, width, height, _, _) = layout(state, cols, rows);
    let visible_w = width.min(cols.saturating_sub(x0));
    let visible_h = height.min(rows.saturating_sub(y0));
    (x0, y0, visible_w, visible_h)
}

fn layout(state: &MenuState, cols: u16, rows: u16) -> (u16, u16, u16, u16, &'static str, String) {
    let hint = if state.path_editor.is_some() {
        "type path · Enter save · Esc cancel · Backspace delete"
    } else {
        "↑↓ select · ←→ change · Enter apply · Space screenshot · Esc close · q quit"
    };
    let hint_cols = hint.chars().count() as u16;
    let footer = format!("v{} · © 2026 terminal-cam", env!("CARGO_PKG_VERSION"),);
    let footer_cols = footer.chars().count() as u16;
    // Width fits the title (50), hint, footer (~28), plus padding.
    let width: u16 = (hint_cols + 6)
        .max(TITLE_COLS + 6)
        .max(footer_cols + 6)
        .max(64);
    // Layout: top (1) + title (3) + blank (1) + items (N) + blank (1)
    //         + hint (1) + footer (1) + bottom (1) = N + 9.
    let height: u16 = (ITEMS.len() as u16) + 9;
    let x0 = cols.saturating_sub(width) / 2;
    let y0 = rows.saturating_sub(height) / 2;
    (x0, y0, width, height, hint, footer)
}

/// Appends the options overlay to `buf`. Caller flushes — drawing into
/// the same buffer as the frame avoids the camera-flash-behind-menu
/// flicker you get from two separate stdout writes.
pub fn draw(state: &MenuState, cfg: &RenderConfig, cols: u16, rows: u16, buf: &mut String) {
    let (x0, y0, width, height, hint, footer) = layout(state, cols, rows);
    let hint_cols = hint.chars().count() as u16;
    let footer_cols = footer.chars().count() as u16;

    buf.push_str("\x1b[0m");

    // Border.
    let _ = write!(
        buf,
        "\x1b[{};{}H╭{}╮",
        y0 + 1,
        x0 + 1,
        "─".repeat(width as usize - 2)
    );
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

    // Title — 3-row half-block ASCII art, centered.
    let tx = x0 + (width - TITLE_COLS) / 2;
    for (offset, line) in [TITLE_LINE_1, TITLE_LINE_2, TITLE_LINE_3].iter().enumerate() {
        let _ = write!(
            buf,
            "\x1b[{};{}H\x1b[1m{}\x1b[0m",
            y0 + 2 + offset as u16,
            tx + 1,
            line
        );
    }

    // Items start one row further down than the 2-row layout to clear the
    // taller title.
    for (i, item) in ITEMS.iter().enumerate() {
        let y = y0 + 6 + i as u16;
        let value = match i {
            0 => state
                .cameras
                .iter()
                .find(|c| c.index == state.current_camera)
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "(none)".into()),
            1 => cfg.style.label().to_string(),
            2 => cfg.mode.label().to_string(),
            3 => {
                if cfg.depth == ColorDepth::Auto {
                    format!("auto · {}", cfg.effective_depth().label())
                } else {
                    cfg.depth.label().to_string()
                }
            }
            4 => {
                if cfg.mirror {
                    "on".into()
                } else {
                    "off".into()
                }
            }
            5 => format!("{:+.2}", cfg.brightness),
            6 => format!("{:.1}", cfg.contrast),
            7 => state
                .path_editor
                .as_ref()
                .map(|value| format!("{value}_"))
                .unwrap_or_else(|| state.screenshot_dir.display().to_string()),
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
            y,
            x0 + 3,
            style,
            trimmed,
            " ".repeat(pad)
        );
    }

    // Hint (penultimate inner row) and version/copyright footer (last inner row).
    let hx = x0 + width.saturating_sub(hint_cols) / 2;
    let _ = write!(
        buf,
        "\x1b[{};{}H\x1b[2m{}\x1b[0m",
        y0 + height - 2,
        hx + 1,
        hint
    );
    let fx = x0 + width.saturating_sub(footer_cols) / 2;
    let _ = write!(
        buf,
        "\x1b[{};{}H\x1b[2m{}\x1b[0m",
        y0 + height - 1,
        fx + 1,
        footer
    );
}
