//! Draws CrossFX's icon from its two SVGs (`crossfx_app::icon`) into the files the tree carries:
//! `assets/icons/apps/crossfx-<size>.png` for every size the Linux packages install, and
//! `assets/windows/crossfx.ico`, the icon `crossfx.exe` carries and both Windows installers show.
//!
//! ```text
//! cargo run -p crossfx-app --example icons
//! cargo run -p crossfx-app --example icons -- --out=DIR
//! ```
//!
//! Run it after changing either drawing in `assets/icons/apps/` and commit what it writes;
//! `tests/icons.rs` fails until then. A build needs neither it nor what it writes: the files are
//! committed. Each size is the drawing meant for it rendered at that size by the resvg CrossFX is
//! built with, never another size resampled, and the output is the same bytes on every run.
//! `--out=DIR` writes the same files under `DIR`, at their paths in the tree, and leaves the tree
//! alone.
//!
//! The `.ico` has a frame for each of `icon::ICO_SIZES`: up to 128 a 32-bit bitmap (a DIB) with
//! its AND mask, the colour rows and the mask rows bottom row first, the colours straight rather
//! than premultiplied, the mask set where a pixel is fully transparent and its rows padded to 32
//! bits; 256 as the very PNG the packages install at that size.

use crossfx_app::icon;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let out = std::env::args()
        .skip(1)
        .find_map(|arg| arg.strip_prefix("--out=").map(PathBuf::from))
        .or_else(|| {
            // The repository: crates/crossfx-app is two levels below it.
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .ancestors()
                .nth(2)
                .map(Path::to_path_buf)
        });
    let Some(out) = out else {
        eprintln!("icons: the repository's folder could not be found; give one with --out=DIR");
        return ExitCode::FAILURE;
    };
    match write_all(&out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("icons: {error}");
            ExitCode::FAILURE
        }
    }
}

fn write_all(out: &Path) -> Result<(), String> {
    let mut pngs = Vec::new();
    for size in icon::SIZES {
        let png = icon::pixmap(size)
            .ok_or_else(|| format!("the icon could not be drawn at {size} px"))?
            .encode_png()
            .map_err(|error| format!("the {size} px icon could not be encoded: {error}"))?;
        write(
            &in_tree(out, &format!("assets/icons/apps/crossfx-{size}.png")),
            &png,
        )?;
        pngs.push((size, png));
    }

    let mut frames = Vec::new();
    for size in icon::ICO_SIZES {
        let frame = if size >= icon::ICO_PNG_FROM {
            pngs.iter()
                .find(|(png_size, _)| *png_size == size)
                .map(|(_, png)| png.clone())
                .ok_or_else(|| format!("no {size} px PNG for the .ico"))?
        } else {
            dib(&icon::pixmap(size).ok_or_else(|| format!("no {size} px frame for the .ico"))?)
        };
        frames.push((size, frame));
    }
    write(&in_tree(out, "assets/windows/crossfx.ico"), &ico(&frames)?)
}

/// `relative`, a path in the tree spelled with `/`, under `out`.
fn in_tree(out: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(out.to_path_buf(), |path, part| path.join(part))
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("{} could not be made: {error}", dir.display()))?;
    }
    std::fs::write(path, bytes)
        .map_err(|error| format!("{} could not be written: {error}", path.display()))?;
    println!("{} ({} bytes)", path.display(), bytes.len());
    Ok(())
}

/// A frame as an icon DIB: a `BITMAPINFOHEADER` whose height counts the colour rows and the mask
/// rows, the colour rows as B, G, R, A, then the 1-bit AND mask.
fn dib(pixmap: &tiny_skia::Pixmap) -> Vec<u8> {
    let (width, height) = (pixmap.width(), pixmap.height());
    let mask_row = width.div_ceil(32) * 4;
    let image_size = width * height * 4 + mask_row * height;
    let mut out = Vec::new();
    out.extend_from_slice(&40_u32.to_le_bytes()); // biSize
    out.extend_from_slice(&width.to_le_bytes()); // biWidth
    out.extend_from_slice(&(height * 2).to_le_bytes()); // biHeight: the colours and the mask
    out.extend_from_slice(&1_u16.to_le_bytes()); // biPlanes
    out.extend_from_slice(&32_u16.to_le_bytes()); // biBitCount
    out.extend_from_slice(&0_u32.to_le_bytes()); // biCompression: BI_RGB
    out.extend_from_slice(&image_size.to_le_bytes()); // biSizeImage
    out.extend_from_slice(&[0; 16]); // resolution, colours used, colours important
    let pixels = pixmap.pixels();
    let row = |y: u32| {
        let start = (y * width) as usize;
        &pixels[start..start + width as usize]
    };
    for y in (0..height).rev() {
        for pixel in row(y) {
            let colour = pixel.demultiply();
            out.extend_from_slice(&[colour.blue(), colour.green(), colour.red(), colour.alpha()]);
        }
    }
    for y in (0..height).rev() {
        let mut mask = vec![0_u8; mask_row as usize];
        for (x, pixel) in row(y).iter().enumerate() {
            if pixel.alpha() == 0 {
                mask[x / 8] |= 0x80 >> (x % 8);
            }
        }
        out.extend_from_slice(&mask);
    }
    out
}

/// The `.ico` file: its header, an entry for each frame, then the frames in the same order.
fn ico(frames: &[(u32, Vec<u8>)]) -> Result<Vec<u8>, String> {
    let count = u16::try_from(frames.len()).map_err(|_| "too many frames".to_owned())?;
    let mut out = Vec::new();
    out.extend_from_slice(&0_u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1_u16.to_le_bytes()); // type: icon
    out.extend_from_slice(&count.to_le_bytes());
    let mut offset = 6 + 16 * u32::from(count);
    for (size, frame) in frames {
        // One byte each for the width and the height, where 0 stands for 256.
        let edge = u8::try_from(*size % 256).map_err(|_| format!("a {size} px frame"))?;
        let length = u32::try_from(frame.len()).map_err(|_| format!("a {size} px frame"))?;
        out.extend_from_slice(&[edge, edge, 0, 0]); // width, height, no palette, reserved
        out.extend_from_slice(&1_u16.to_le_bytes()); // planes
        out.extend_from_slice(&32_u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += length;
    }
    for (_, frame) in frames {
        out.extend_from_slice(frame);
    }
    Ok(out)
}
