# terminal-cam

Live webcam → ASCII art in your terminal. 95-char density ramp, truecolor
output, painterly color styles, optional solid-block rendering, and a few
fractal-driven psychedelic modes.

![styles](https://img.shields.io/badge/styles-7-blue) ![render](https://img.shields.io/badge/render-ASCII%20%7C%20Blocks-green) ![rust](https://img.shields.io/badge/rust-2021-orange) ![license](https://img.shields.io/badge/license-MIT-lightgrey)

## Install

```sh
cargo install --git https://github.com/bluevisor/terminal-cam
```

or clone and run:

```sh
git clone https://github.com/bluevisor/terminal-cam
cd terminal-cam
cargo run --release
```

macOS will prompt for camera permission on first launch — grant it to your
terminal application.

## Controls

| Key            | Action              |
| -------------- | ------------------- |
| `Esc`          | Open / close menu   |
| `↑` `↓`        | Select menu item    |
| `←` `→`        | Change value        |
| `Enter`        | Apply / advance     |
| `Space`        | Save screenshot     |
| `q` / `Ctrl-C` | Quit                |

## Options

- **Camera source** — cycle detected cameras
- **Style** — `Color`, `B&W`, `Sepia`, `Van Gogh`, `Monet`, `Mushroom`, `LSD`
  - `Van Gogh` — static palette snap. Source hue picks one of three ramps
    (cool / warm / green), source luma picks the anchor within it.
    Hand-curated from Starry Night, Irises, and the self-portrait.
  - `Monet` — pastelized HSV with a slow dappled-light mottle and
    atmospheric warm/cool shift.
  - `Mushroom` — radial mandala (concentric rings + 6-fold angular petals)
    plus a slow-drifting Julia-set iteration field.
  - `LSD` — Julia-set iteration count drives hue rotation; the c-parameter
    drifts slowly so the fractal morphs through related shapes.
- **Render mode** — `ASCII` (density-ramp glyphs) or `Blocks` (solid `█`
  in color styles so the color escape alone carries the image; `░▒▓█`
  shading ramp in B&W).
- **Color depth** — `auto` / `truecolor` / `256` / `16`. Auto-detects from
  `COLORTERM`; falls back to 256-color for terminals that don't speak
  24-bit.
- **Mirror** — horizontal flip (on by default; webcams are usually mirrored).
- **Brightness** — `-1.00` to `+1.00`, added to each RGB channel pre-style.
- **Contrast** — `0.1` to `3.0`. Applied to RGB upstream (not just to glyph
  density) so it affects the emitted color, Van Gogh palette-band
  selection, and Mushroom / LSD HSV value.
- **Screenshot path** — directory used when pressing `Space`. Press `Enter`
  or `→` on the row, type a path, then press `Enter` to save it to the app
  config. `~` is expanded to your home directory.

Screenshots are saved as PNG files using the current terminal crop, mirror,
style, brightness, contrast, and render mode settings.

## Terminal compatibility

| Terminal                                                  | Auto-detects as |
| --------------------------------------------------------- | --------------- |
| iTerm2, Warp, Kitty, Alacritty, Ghostty, WezTerm, VS Code | truecolor       |
| Apple Terminal.app                                        | 256             |
| anything with `TERM=*-256color`                           | 256             |

If colors look wrong, the terminal probably doesn't support truecolor —
open the menu and set **Color depth** to `256`. The image will show some
banding but the colors will be correct.

## How it works

Per terminal cell, every frame:

1. Average RGB over the source block.
2. Add brightness offset, apply contrast stretch around mid-gray (128).
3. Run the style transform. Palette / fractal styles produce new RGB from
   source luma + hue (Van Gogh), HSV rotation (Monet / Mushroom / LSD),
   or an affine matrix (Sepia).
4. Recompute luminance from the stylized RGB.
5. Pick a glyph:
   - **ASCII**: sigmoid-shaped luma indexes the 95-char density ramp.
   - **Blocks** (color): solid `█` — color escape carries brightness.
   - **Blocks** (B&W): `░▒▓█` shading ramp, since there's no color channel.
6. Quantize the fg color to the selected depth (truecolor / 256-palette /
   ANSI-16) and emit the escape + glyph.

Each frame is wrapped in DEC 2026 synchronized-update markers
(`\x1b[?2026h` / `\x1b[?2026l`) so supporting terminals paint atomically.
The options overlay is composed into the same byte buffer as the render
and flushed in one write — no camera-flash-behind-menu flicker.

Aspect ratio is preserved via center-crop: the source is cropped to match
the terminal grid's pixel canvas (`cols : rows × CHAR_ASPECT`) so faces
don't squash or stretch as you resize the window.

## Project structure

- `src/main.rs` — input loop, terminal mode setup, frame pacing
- `src/camera.rs` — `nokhwa` capture thread + shared frame slot
- `src/render.rs` — per-cell render pipeline, config, synchronized output
- `src/screenshot.rs` — screenshot capture + dependency-free PNG writing
- `src/config.rs` — screenshot path config load/save
- `src/style.rs` — style transforms (Sepia / Van Gogh / Monet / Mushroom /
  LSD) and the Julia-set iteration helper
- `src/color.rs` — depth detection + truecolor / 256 / ANSI-16 quantization
- `src/ascii.rs` — 95-char density ramp and 5-stop shading ramp
- `src/menu.rs` — centered options overlay (half-block title, version
  footer, rendered into the render buffer so it composites atomically)

## Tuning

The terminal cell aspect ratio (`CHAR_ASPECT = 2.0` in `src/render.rs`)
is the one font-dependent constant. If circles render as horizontal
ovals, bump it; if vertical ovals, drop it. iTerm2 / Terminal.app / Kitty
with default fonts are all close to 2.0.

Van Gogh's three palette ramps live at the top of `src/style.rs` as
`VG_COOL` / `VG_WARM` / `VG_GREEN`. Each is a 5-anchor ramp (dark →
light); swap in your own anchors for a different painter.

Mushroom and LSD's Julia `c` parameter drifts on a small loop — tune
speed, amplitude, and center in `mushroom()` / `lsd()` in `src/style.rs`
to explore different regions of the fractal parameter space.

## Stack

- [`nokhwa`](https://crates.io/crates/nokhwa) — cross-platform camera capture
- [`crossterm`](https://crates.io/crates/crossterm) — terminal I/O
- [`parking_lot`](https://crates.io/crates/parking_lot) — faster mutex for the frame slot
- [`anyhow`](https://crates.io/crates/anyhow) — error handling

## License

MIT — see [`LICENSE`](LICENSE).
