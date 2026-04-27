//! Screenshot capture and minimal PNG encoding.

use crate::{camera::Frame, render};
use anyhow::{anyhow, Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn save(
    frame: &Frame,
    cols: u16,
    rows: u16,
    cfg: &render::RenderConfig,
    time: f32,
    dir: &Path,
) -> Result<PathBuf> {
    fs::create_dir_all(dir)
        .with_context(|| format!("create screenshot directory {}", dir.display()))?;

    let image = render::render_screenshot(frame, cols, rows, cfg, time)
        .ok_or_else(|| anyhow!("terminal size is too small for a screenshot"))?;
    let path = dir.join(screenshot_name()?);
    write_png(&path, image.width, image.height, &image.rgb)?;
    Ok(path)
}

fn screenshot_name() -> Result<String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time is before unix epoch")?;
    Ok(format!(
        "terminal-cam-{}-{:03}.png",
        now.as_secs(),
        now.subsec_millis()
    ))
}

fn write_png(path: &Path, width: u32, height: u32, rgb: &[u8]) -> Result<()> {
    if rgb.len() != width as usize * height as usize * 3 {
        return Err(anyhow!("invalid screenshot buffer length"));
    }

    let mut out = Vec::with_capacity(rgb.len() + height as usize + 256);
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(2); // truecolor RGB
    ihdr.push(0); // compression method
    ihdr.push(0); // filter method
    ihdr.push(0); // no interlace
    write_chunk(&mut out, b"IHDR", &ihdr);

    let mut scanlines = Vec::with_capacity((width as usize * 3 + 1) * height as usize);
    let row_len = width as usize * 3;
    for row in rgb.chunks_exact(row_len) {
        scanlines.push(0); // PNG filter type 0: None
        scanlines.extend_from_slice(row);
    }

    let mut zlib = Vec::with_capacity(scanlines.len() + scanlines.len() / 65_535 * 5 + 6);
    zlib.extend_from_slice(&[0x78, 0x01]); // zlib header: deflate, fastest/no compression
    write_stored_deflate_blocks(&mut zlib, &scanlines);
    zlib.extend_from_slice(&adler32(&scanlines).to_be_bytes());
    write_chunk(&mut out, b"IDAT", &zlib);
    write_chunk(&mut out, b"IEND", &[]);

    fs::write(path, out).with_context(|| format!("write screenshot {}", path.display()))
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);

    let mut crc_input = Vec::with_capacity(kind.len() + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

fn write_stored_deflate_blocks(out: &mut Vec<u8>, data: &[u8]) {
    if data.is_empty() {
        out.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
        return;
    }

    let mut remaining = data;
    while !remaining.is_empty() {
        let block_len = remaining.len().min(65_535);
        let final_block = block_len == remaining.len();
        out.push(if final_block { 1 } else { 0 });

        let len = block_len as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(&remaining[..block_len]);
        remaining = &remaining[block_len..];
    }
}

fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let mut a = 1u32;
    let mut b = 0u32;

    for &byte in data {
        a = (a + byte as u32) % MOD;
        b = (b + a) % MOD;
    }

    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, process};

    #[test]
    fn writes_png_container() -> Result<()> {
        let path = env::temp_dir().join(format!("terminal-cam-test-{}.png", process::id()));

        write_png(&path, 1, 1, &[255, 0, 0])?;
        let bytes = fs::read(&path)?;
        let _ = fs::remove_file(&path);

        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(bytes.windows(4).any(|window| window == b"IHDR"));
        assert!(bytes.windows(4).any(|window| window == b"IDAT"));
        assert!(bytes.windows(4).any(|window| window == b"IEND"));
        Ok(())
    }
}
