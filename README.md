# terminal-cam

ASCII-art webcam viewer for your terminal. Renders live camera video using
all 95 printable ASCII characters sorted by ink density, with truecolor output
and a handful of painterly color styles.

![styles](https://img.shields.io/badge/styles-7-blue) ![rust](https://img.shields.io/badge/rust-2021-orange)

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

| Key          | Action              |
| ------------ | ------------------- |
| `Esc`        | Open / close menu   |
| `↑` `↓`      | Select menu item    |
| `←` `→`      | Change value        |
| `Enter`      | Apply / advance     |
| `q` / `Ctrl-C` | Quit              |

## Options menu

- **Camera source** — cycle detected cameras
- **Style** — `Full Color`, `B&W`, `Sepia`, `Van Gogh`, `Monet`, `Mushroom`, `LSD`
- **Mirror** — horizontal flip (on by default; webcams are usually mirrored)
- **Brightness** — `-1.00` to `+1.00`, added to each RGB channel pre-style
- **Contrast** — `0.1` to `3.0`, applied around 0.5 before the glyph curve

## How it works

Per terminal cell:

1. Average RGB over the source block.
2. Add brightness offset.
3. Apply style transform — palette styles (Sepia, Van Gogh, Monet) map
   luminance into a 5-stop gradient, throwing away source hue. Trippy
   styles (Mushroom, LSD) rotate hue in HSV over time and cell position.
4. Recompute luminance from the stylized RGB.
5. Contrast-shape via `(l - 0.5) * contrast + 0.5`, then run through a
   sigmoid. Color mode uses steep sigmoid (k=12) since truecolor carries
   brightness; B&W uses gentler (k=5) to preserve midtone gradations.
6. Index the 95-char density ramp, emit truecolor fg escape + glyph.

Aspect ratio is preserved via center-crop: the source is cropped to match
the terminal grid's pixel canvas (`cols : rows × CHAR_ASPECT`) so faces
don't squash or stretch as you resize the window.

## Tuning

The terminal cell aspect ratio (`CHAR_ASPECT = 2.0` in `src/render.rs`) is
the one font-dependent constant. If circles render as horizontal ovals, bump
it; if vertical ovals, drop it. iTerm2 / Terminal.app / Kitty with default
fonts are all close to 2.0.

The ASCII ramp lives in `src/ascii.rs`. Swap the sigmoid curve or steepness
to change the feel — a short comment in `luma_to_char` lays out alternatives
(linear, gamma, s-curve).

Style palettes are in `src/style.rs` as 5-stop gradients. Add your own
style by adding a variant to the `Style` enum, dropping a gradient next to
`VAN_GOGH`, and handling it in `transform()`.

## Stack

- [`nokhwa`](https://crates.io/crates/nokhwa) — cross-platform camera capture
- [`crossterm`](https://crates.io/crates/crossterm) — terminal I/O
- [`parking_lot`](https://crates.io/crates/parking_lot) — faster mutex for the frame slot
- [`anyhow`](https://crates.io/crates/anyhow) — error handling

## License

MIT
