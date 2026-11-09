//! Persisted app settings: a small `key=value` file, written whenever a
//! setting changes so the next launch picks up where you left off.

use crate::{
    color::ColorDepth,
    render::{RenderConfig, RenderMode},
    style::Style,
    vcam::VcamDetail,
};
use anyhow::{anyhow, Context, Result};
use std::{
    env,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

const APP_DIR: &str = "terminal-cam";
const CONFIG_FILE: &str = "config";

#[derive(Clone, Debug, PartialEq)]
pub struct AppConfig {
    pub screenshot_dir: PathBuf,
    pub style: Style,
    pub mode: RenderMode,
    pub depth: ColorDepth,
    pub mirror: bool,
    pub brightness: f32,
    pub contrast: f32,
    pub virtual_cam: bool,
    pub vcam_detail: VcamDetail,
    /// Name of the last camera used, so multi-camera Macs skip the prompt.
    pub camera: Option<String>,
    path: PathBuf,
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        let path = config_file_path()?;
        let defaults = RenderConfig::default();
        let mut cfg = Self {
            screenshot_dir: default_screenshot_dir()?,
            style: defaults.style,
            mode: defaults.mode,
            depth: defaults.depth,
            mirror: defaults.mirror,
            brightness: defaults.brightness,
            contrast: defaults.contrast,
            virtual_cam: false,
            vcam_detail: VcamDetail::Normal,
            camera: None,
            path,
        };

        if cfg.path.exists() {
            let raw = fs::read_to_string(&cfg.path)
                .with_context(|| format!("read config {}", cfg.path.display()))?;
            cfg.apply_config_text(&raw)?;
        }

        Ok(cfg)
    }

    pub fn set_screenshot_dir_from_input(&mut self, input: &str) -> Result<()> {
        self.screenshot_dir = normalize_dir_input(input)?;
        Ok(())
    }

    /// Copies the render settings onto `cfg`, leaving detection alone.
    pub fn apply_to(&self, cfg: &mut RenderConfig) {
        cfg.style = self.style;
        cfg.mode = self.mode;
        cfg.depth = self.depth;
        cfg.mirror = self.mirror;
        cfg.brightness = self.brightness;
        cfg.contrast = self.contrast;
    }

    /// Captures the live render settings so they can be compared and saved.
    pub fn capture_from(&mut self, cfg: &RenderConfig) {
        self.style = cfg.style;
        self.mode = cfg.mode;
        self.depth = cfg.depth;
        self.mirror = cfg.mirror;
        self.brightness = cfg.brightness;
        self.contrast = cfg.contrast;
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create config directory {}", parent.display()))?;
        }
        fs::write(&self.path, self.to_text())
            .with_context(|| format!("write config {}", self.path.display()))
    }

    fn to_text(&self) -> String {
        let on_off = |b: bool| if b { "on" } else { "off" };
        let mut body =
            String::from("# terminal-cam settings (rewritten when you change a setting)\n");
        let _ = writeln!(body, "screenshot_dir={}", self.screenshot_dir.display());
        let _ = writeln!(body, "style={}", self.style.slug());
        let _ = writeln!(body, "mode={}", self.mode.label().to_ascii_lowercase());
        let _ = writeln!(body, "depth={}", self.depth.label());
        let _ = writeln!(body, "mirror={}", on_off(self.mirror));
        let _ = writeln!(body, "brightness={:.2}", self.brightness);
        let _ = writeln!(body, "contrast={:.1}", self.contrast);
        let _ = writeln!(body, "virtual_cam={}", on_off(self.virtual_cam));
        let _ = writeln!(body, "vcam_detail={}", self.vcam_detail.label());
        if let Some(camera) = &self.camera {
            let _ = writeln!(body, "camera={camera}");
        }
        body
    }

    /// Unknown keys and unparseable values are skipped (keeping the default)
    /// so a hand-edit or an older/newer version never blocks startup.
    fn apply_config_text(&mut self, raw: &str) -> Result<()> {
        for (line_no, line) in raw.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let Some((key, value)) = line.split_once('=') else {
                return Err(anyhow!(
                    "invalid config line {} in {}",
                    line_no + 1,
                    self.path.display()
                ));
            };
            let value = value.trim();

            match key.trim() {
                "screenshot_dir" => self.screenshot_dir = normalize_dir_input(value)?,
                "style" => set_some(&mut self.style, Style::from_name(value)),
                "mode" => set_some(&mut self.mode, RenderMode::from_name(value)),
                "depth" => set_some(&mut self.depth, ColorDepth::from_name(value)),
                "mirror" => set_some(&mut self.mirror, parse_bool(value)),
                "brightness" => set_some(
                    &mut self.brightness,
                    value.parse().ok().map(|v: f32| v.clamp(-1.0, 1.0)),
                ),
                "contrast" => set_some(
                    &mut self.contrast,
                    value.parse().ok().map(|v: f32| v.clamp(0.1, 3.0)),
                ),
                "virtual_cam" => set_some(&mut self.virtual_cam, parse_bool(value)),
                "vcam_detail" => set_some(&mut self.vcam_detail, VcamDetail::from_name(value)),
                "camera" if !value.is_empty() => self.camera = Some(value.to_string()),
                _ => {}
            }
        }

        Ok(())
    }
}

fn set_some<T>(slot: &mut T, value: Option<T>) {
    if let Some(v) = value {
        *slot = v;
    }
}

pub fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => Some(true),
        "off" | "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

pub fn normalize_dir_input(input: &str) -> Result<PathBuf> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("screenshot path cannot be empty"));
    }
    if trimmed.contains('\0') || trimmed.contains('\n') || trimmed.contains('\r') {
        return Err(anyhow!("screenshot path contains an invalid character"));
    }

    let expanded = expand_home(trimmed)?;
    let path = PathBuf::from(expanded);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(env::current_dir()
            .context("resolve current directory")?
            .join(path))
    }
}

fn expand_home(input: &str) -> Result<String> {
    if input == "~" {
        return Ok(home_dir()?.display().to_string());
    }

    if let Some(rest) = input.strip_prefix("~/") {
        return Ok(home_dir()?.join(rest).display().to_string());
    }

    Ok(input.to_string())
}

fn default_screenshot_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join("Pictures").join(APP_DIR))
}

fn config_file_path() -> Result<PathBuf> {
    if let Some(base) = env::var_os("XDG_CONFIG_HOME") {
        return Ok(Path::new(&base).join(APP_DIR).join(CONFIG_FILE));
    }

    #[cfg(target_os = "macos")]
    {
        Ok(home_dir()?
            .join("Library")
            .join("Application Support")
            .join(APP_DIR)
            .join(CONFIG_FILE))
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(base) = env::var_os("APPDATA") {
            return Ok(Path::new(&base).join(APP_DIR).join(CONFIG_FILE));
        }
        Ok(home_dir()?.join(".config").join(APP_DIR).join(CONFIG_FILE))
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Ok(home_dir()?.join(".config").join(APP_DIR).join(CONFIG_FILE))
    }
}

fn home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| anyhow!("HOME is not set"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank() -> AppConfig {
        let d = RenderConfig::default();
        AppConfig {
            screenshot_dir: PathBuf::from("/tmp/shots"),
            style: d.style,
            mode: d.mode,
            depth: d.depth,
            mirror: d.mirror,
            brightness: d.brightness,
            contrast: d.contrast,
            virtual_cam: false,
            vcam_detail: VcamDetail::Normal,
            camera: None,
            path: PathBuf::from("/tmp/terminal-cam-test-config"),
        }
    }

    #[test]
    fn settings_round_trip_through_text() {
        let mut cfg = blank();
        cfg.style = Style::VanGogh;
        cfg.mode = RenderMode::Blocks;
        cfg.depth = ColorDepth::Palette256;
        cfg.mirror = false;
        cfg.brightness = -0.25;
        cfg.contrast = 1.5;
        cfg.virtual_cam = true;
        cfg.vcam_detail = VcamDetail::Chunky;
        cfg.camera = Some("FaceTime HD Camera".into());

        let mut back = blank();
        back.apply_config_text(&cfg.to_text()).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn bad_values_and_unknown_keys_keep_defaults() {
        let mut cfg = blank();
        cfg.apply_config_text("style=plaid\ncontrast=lots\nfuture_key=1\nmirror=off\n")
            .unwrap();
        assert_eq!(cfg.style, RenderConfig::default().style);
        assert_eq!(cfg.contrast, RenderConfig::default().contrast);
        assert!(!cfg.mirror);
    }

    #[test]
    fn old_screenshot_only_config_still_loads() {
        let mut cfg = blank();
        cfg.apply_config_text("screenshot_dir=/var/tmp\n").unwrap();
        assert_eq!(cfg.screenshot_dir, PathBuf::from("/var/tmp"));
        assert_eq!(cfg.style, RenderConfig::default().style);
    }
}
