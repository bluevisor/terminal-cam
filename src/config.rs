//! Minimal app config persisted outside the render loop.

use anyhow::{anyhow, Context, Result};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

const APP_DIR: &str = "terminal-cam";
const CONFIG_FILE: &str = "config";
const SCREENSHOT_DIR_KEY: &str = "screenshot_dir";

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub screenshot_dir: PathBuf,
    path: PathBuf,
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        let path = config_file_path()?;
        let mut cfg = Self {
            screenshot_dir: default_screenshot_dir()?,
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

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create config directory {}", parent.display()))?;
        }

        let body = format!("{}={}\n", SCREENSHOT_DIR_KEY, self.screenshot_dir.display());
        fs::write(&self.path, body).with_context(|| format!("write config {}", self.path.display()))
    }

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

            if key.trim() == SCREENSHOT_DIR_KEY {
                self.screenshot_dir = normalize_dir_input(value.trim())?;
            }
        }

        Ok(())
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
