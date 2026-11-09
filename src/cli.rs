//! Command-line flags. Hand-rolled to keep the dependency list short.
//!
//! Precedence: built-in defaults < saved config < these flags.

use crate::{
    color::ColorDepth, config::parse_bool, render::RenderMode, style, style::Style,
    vcam::VcamDetail,
};
use anyhow::{anyhow, bail, Result};

#[derive(Debug, Default, PartialEq)]
pub struct Args {
    pub style: Option<Style>,
    pub mode: Option<RenderMode>,
    pub depth: Option<ColorDepth>,
    pub mirror: Option<bool>,
    pub brightness: Option<f32>,
    pub contrast: Option<f32>,
    pub camera: Option<String>,
    pub virtual_cam: Option<bool>,
    pub vcam_detail: Option<VcamDetail>,
    pub command: Command,
}

#[derive(Debug, Default, PartialEq)]
pub enum Command {
    #[default]
    Run,
    Help,
    Version,
    ListCameras,
    ListStyles,
}

pub const HELP: &str = "\
terminal-cam — live webcam as ASCII art in your terminal

USAGE:
    terminal-cam [OPTIONS]

OPTIONS:
    -s, --style <NAME>       Color style (see --list-styles)
    -m, --mode <MODE>        ascii | blocks
    -d, --depth <DEPTH>      auto | truecolor | 256 | 16
        --mirror / --no-mirror
                             Flip horizontally (default: on)
    -b, --brightness <N>     -1.0 to 1.0
    -c, --contrast <N>       0.1 to 3.0
        --camera <N|NAME>    Camera number from --list-cameras, or part of its name
        --virtual-cam / --no-virtual-cam
                             Publish to the \"Terminal Cam\" macOS camera
        --vcam-detail <D>    chunky | coarse | normal | fine
        --list-cameras       Print cameras and exit
        --list-styles        Print styles and exit
    -h, --help               Print help
    -V, --version            Print version

Settings you change in the app are remembered for next launch; flags
override them for this run.

KEYS:
    Esc menu · ←/→ style · m mode · r record GIF · v virtual cam
    Space screenshot · q quit
";

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Args> {
    let mut out = Args::default();
    let mut it = args.into_iter();

    while let Some(raw) = it.next() {
        // Accept both `--style sepia` and `--style=sepia`.
        let (flag, inline) = match raw.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (raw.clone(), None),
        };
        let mut value = |name: &str| -> Result<String> {
            inline
                .clone()
                .or_else(|| it.next())
                .ok_or_else(|| anyhow!("{name} needs a value"))
        };

        match flag.as_str() {
            "-h" | "--help" => out.command = Command::Help,
            "-V" | "--version" => out.command = Command::Version,
            "--list-cameras" => out.command = Command::ListCameras,
            "--list-styles" => out.command = Command::ListStyles,
            "-s" | "--style" => {
                let v = value("--style")?;
                out.style = Some(
                    Style::from_name(&v)
                        .ok_or_else(|| anyhow!("unknown style {v:?} (try --list-styles)"))?,
                );
            }
            "-m" | "--mode" => {
                let v = value("--mode")?;
                out.mode = Some(
                    RenderMode::from_name(&v)
                        .ok_or_else(|| anyhow!("unknown mode {v:?} (ascii | blocks)"))?,
                );
            }
            "-d" | "--depth" => {
                let v = value("--depth")?;
                out.depth = Some(ColorDepth::from_name(&v).ok_or_else(|| {
                    anyhow!("unknown color depth {v:?} (auto | truecolor | 256 | 16)")
                })?);
            }
            "--mirror" => {
                out.mirror = Some(match inline {
                    Some(v) => parse_bool(&v).ok_or_else(|| anyhow!("--mirror=on|off"))?,
                    None => true,
                })
            }
            "--no-mirror" => out.mirror = Some(false),
            "-b" | "--brightness" => {
                out.brightness = Some(number(&value("--brightness")?, -1.0, 1.0, "--brightness")?)
            }
            "-c" | "--contrast" => {
                out.contrast = Some(number(&value("--contrast")?, 0.1, 3.0, "--contrast")?)
            }
            "--camera" => out.camera = Some(value("--camera")?),
            "--virtual-cam" => out.virtual_cam = Some(true),
            "--no-virtual-cam" => out.virtual_cam = Some(false),
            "--vcam-detail" => {
                let v = value("--vcam-detail")?;
                out.vcam_detail = Some(VcamDetail::from_name(&v).ok_or_else(|| {
                    anyhow!("unknown detail {v:?} (chunky | coarse | normal | fine)")
                })?);
            }
            other => bail!("unknown option {other:?} (try --help)"),
        }
    }

    Ok(out)
}

fn number(raw: &str, min: f32, max: f32, name: &str) -> Result<f32> {
    let v: f32 = raw
        .parse()
        .map_err(|_| anyhow!("{name} expects a number, got {raw:?}"))?;
    if !(min..=max).contains(&v) {
        bail!("{name} must be between {min} and {max}");
    }
    Ok(v)
}

pub fn styles_listing() -> String {
    style::ALL
        .iter()
        .map(|s| format!("  {:<10} {}\n", s.slug(), s.label()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Args> {
        parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_values_in_both_forms() {
        let a = p(&[
            "--style",
            "van-gogh",
            "--mode=blocks",
            "-c",
            "1.5",
            "--no-mirror",
        ])
        .unwrap();
        assert_eq!(a.style, Some(Style::VanGogh));
        assert_eq!(a.mode, Some(RenderMode::Blocks));
        assert_eq!(a.contrast, Some(1.5));
        assert_eq!(a.mirror, Some(false));
        assert_eq!(a.command, Command::Run);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(p(&["--style", "plaid"]).is_err());
        assert!(p(&["--contrast", "9"]).is_err());
        assert!(p(&["--brightness"]).is_err());
        assert!(p(&["--wat"]).is_err());
    }

    #[test]
    fn virtual_cam_flags() {
        let a = p(&["--virtual-cam", "--vcam-detail", "chunky"]).unwrap();
        assert_eq!(a.virtual_cam, Some(true));
        assert_eq!(a.vcam_detail, Some(VcamDetail::Chunky));
    }
}
