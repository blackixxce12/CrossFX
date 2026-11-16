//! CrossFX's icon as the tree carries it is what its two drawings draw (`crossfx_app::icon`): the
//! PNG of every size the Linux packages install, and every frame of `crossfx.exe`'s icon. A
//! drawing changed without `cargo run -p crossfx-app --example icons` after it fails here, and so
//! does a bitmap resampled from another size instead of drawn at its own. And the tray's three
//! icons are its wave-X, one colour per state.

use crossfx_app::icon;
use crossfx_app::tray::{TrayIcon, rasterise_rgba};
use std::fs;
use std::path::{Path, PathBuf};

/// The repository this test binary was built from.
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root exists")
}

fn read(relative: &str) -> Vec<u8> {
    let path = repo().join(relative);
    fs::read(&path).unwrap_or_else(|error| panic!("{} could not be read: {error}", path.display()))
}

/// How far a channel of a pixel, premultiplied, may stray from the drawing. A PNG keeps its
/// colours straight, and making them premultiplied again rounds each by one at most; one more
/// leaves room for a processor that rounds the drawing's own arithmetic otherwise.
const TOLERANCE: u8 = 2;

/// The first pixel of `pixels`, premultiplied RGBA of a `size` × `size` square, that is not the
/// icon drawn at that size, as where it is and what it holds; `None` when every pixel is.
fn first_difference(pixels: &[u8], size: u32) -> Option<String> {
    let drawn = icon::pixmap(size).expect("the icon is drawn");
    let drawn = drawn.data();
    if pixels.len() != drawn.len() {
        return Some(format!(
            "{} bytes of pixels for {}",
            pixels.len(),
            drawn.len()
        ));
    }
    pixels
        .chunks(4)
        .zip(drawn.chunks(4))
        .position(|(got, want)| {
            got.iter()
                .zip(want)
                .any(|(g, w)| g.abs_diff(*w) > TOLERANCE)
        })
        .map(|at| {
            let (x, y) = (at % size as usize, at / size as usize);
            let span = at * 4..at * 4 + 4;
            format!(
                "({x}, {y}) is {:?} where the drawing has {:?}",
                &pixels[span.clone()],
                &drawn[span]
            )
        })
}

/// The drawing `size` comes from, for a message.
fn drawing(size: u32) -> &'static str {
    if size <= icon::SMALL_UP_TO {
        "crossfx-small.svg"
    } else {
        "crossfx.svg"
    }
}

#[test]
fn every_png_of_the_icon_is_its_drawing_rendered_at_its_size() {
    for size in icon::SIZES {
        let path = format!("assets/icons/apps/crossfx-{size}.png");
        let png = tiny_skia::Pixmap::decode_png(&read(&path))
            .unwrap_or_else(|error| panic!("{path} is not a PNG: {error}"));
        assert_eq!((png.width(), png.height()), (size, size), "{path}");
        if let Some(wrong) = first_difference(png.data(), size) {
            panic!(
                "{path} is not {} rendered at {size} px: {wrong}. Run `cargo run -p crossfx-app \
                 --example icons` and commit what it writes",
                drawing(size)
            );
        }
    }
}

#[test]
fn the_icons_folder_holds_the_two_drawings_and_a_png_of_each_size_and_nothing_else() {
    let mut found: Vec<String> = fs::read_dir(repo().join("assets/icons/apps"))
        .expect("assets/icons/apps is listed")
        .map(|entry| {
            entry
                .expect("an entry of assets/icons/apps")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    found.sort();
    let mut expected: Vec<String> = icon::SIZES
        .iter()
        .map(|size| format!("crossfx-{size}.png"))
        .chain(["crossfx.svg".to_owned(), "crossfx-small.svg".to_owned()])
        .collect();
    expected.sort();
    assert_eq!(found, expected);
    // And the drawings the binary carries are those files.
    assert_eq!(icon::SVG, read("assets/icons/apps/crossfx.svg"));
    assert_eq!(icon::SMALL_SVG, read("assets/icons/apps/crossfx-small.svg"));
}

#[test]
fn the_windows_icon_has_a_frame_of_every_size_drawn_at_that_size() {
    let ico = read("assets/windows/crossfx.ico");
    let u16_at = |at: usize| u16::from_le_bytes([ico[at], ico[at + 1]]);
    let u32_at =
        |at: usize| u32::from_le_bytes([ico[at], ico[at + 1], ico[at + 2], ico[at + 3]]) as usize;
    assert_eq!(u16_at(0), 0, "reserved");
    assert_eq!(u16_at(2), 1, "an icon, not a cursor");
    assert_eq!(usize::from(u16_at(4)), icon::ICO_SIZES.len(), "frames");

    let mut end = 6 + 16 * icon::ICO_SIZES.len();
    for (index, size) in icon::ICO_SIZES.into_iter().enumerate() {
        let entry = 6 + 16 * index;
        // One byte each for the width and the height, where 0 stands for 256.
        let edge = u8::try_from(size % 256).expect("a byte");
        assert_eq!(
            ico[entry..entry + 4],
            [edge, edge, 0, 0],
            "{size}: the width, the height, no palette"
        );
        assert_eq!(u16_at(entry + 4), 1, "{size}: planes");
        assert_eq!(u16_at(entry + 6), 32, "{size}: bits per pixel");
        let (length, offset) = (u32_at(entry + 8), u32_at(entry + 12));
        assert_eq!(offset, end, "{size}: each frame follows the one before");
        end = offset + length;
        let frame = &ico[offset..end];
        if size >= icon::ICO_PNG_FROM {
            assert!(
                frame == read(&format!("assets/icons/apps/crossfx-{size}.png")),
                "{size}: the frame is not the PNG the packages install"
            );
        } else {
            assert_dib_is_the_drawing(frame, size);
        }
    }
    assert_eq!(end, ico.len(), "something follows the last frame");
}

/// `frame` is a 32-bit icon bitmap of the icon drawn at `size`, with the AND mask that matches it.
fn assert_dib_is_the_drawing(frame: &[u8], size: u32) {
    let u16_at = |at: usize| u16::from_le_bytes([frame[at], frame[at + 1]]);
    let u32_at =
        |at: usize| u32::from_le_bytes([frame[at], frame[at + 1], frame[at + 2], frame[at + 3]]);
    assert_eq!(u32_at(0), 40, "{size}: a BITMAPINFOHEADER");
    assert_eq!(u32_at(4), size, "{size}: the width");
    assert_eq!(u32_at(8), size * 2, "{size}: the height, colours and mask");
    assert_eq!(u16_at(12), 1, "{size}: planes");
    assert_eq!(u16_at(14), 32, "{size}: bits per pixel");
    assert_eq!(u32_at(16), 0, "{size}: uncompressed");

    let side = size as usize;
    let mask_row = side.div_ceil(32) * 4;
    let colours = &frame[40..40 + side * side * 4];
    let mask = &frame[40 + side * side * 4..];
    assert_eq!(mask.len(), mask_row * side, "{size}: the mask's length");
    assert_eq!(
        u32_at(20) as usize,
        colours.len() + mask.len(),
        "{size}: biSizeImage"
    );

    // The rows run bottom up, each pixel B, G, R, A with its colour straight: turned the right
    // way up and premultiplied, they have to be the drawing's.
    let mut pixels = Vec::with_capacity(side * side * 4);
    for y in 0..side {
        let row = side - 1 - y;
        for x in 0..side {
            let at = (row * side + x) * 4;
            let (b, g, r, a) = (
                colours[at],
                colours[at + 1],
                colours[at + 2],
                colours[at + 3],
            );
            let colour = tiny_skia::ColorU8::from_rgba(r, g, b, a).premultiply();
            pixels.extend_from_slice(&[
                colour.red(),
                colour.green(),
                colour.blue(),
                colour.alpha(),
            ]);
            // The AND mask is set exactly where the pixel is fully transparent.
            let masked = mask[row * mask_row + x / 8] & (0x80 >> (x % 8)) != 0;
            assert_eq!(masked, a == 0, "{size}: the mask at ({x}, {y})");
        }
    }
    if let Some(wrong) = first_difference(&pixels, size) {
        panic!(
            "the {size} px frame of assets/windows/crossfx.ico is not {} rendered at {size} px: \
             {wrong}. Run `cargo run -p crossfx-app --example icons` and commit what it writes",
            drawing(size)
        );
    }
}

/// The wave-X of the tray's icons, as their SVGs draw it in a square of 256 units: the diagonal,
/// then the wave.
const WAVE_X: &str =
    "M44 44 L212 212 M44 212 C100.3 212.3 128.3 184.3 128 128 C127.7 71.7 155.7 43.7 212 44";

#[test]
fn every_status_icon_is_the_wave_x_alone_in_its_states_colour() {
    let mut colours = Vec::new();
    for state in TrayIcon::ALL {
        let svg = std::str::from_utf8(state.svg()).expect("utf-8");
        assert!(
            svg.contains(r#"viewBox="0 0 256 256""#),
            "{state:?}: a square, so that no host squeezes the glyph"
        );
        assert_eq!(svg.matches("<path").count(), 1, "{state:?}: one path");
        assert!(
            svg.contains(&format!(r#"d="{WAVE_X}""#)),
            "{state:?}: the wave-X"
        );
        assert!(
            !svg.contains("<rect") && svg.contains(r#"fill="none""#),
            "{state:?}: the glyph alone, on a transparent background"
        );
        let at = svg.find("stroke=\"").expect("a stroke colour") + "stroke=\"".len();
        let colour = svg[at..at + 7].to_owned();

        // Drawn at 32 pixels: clear in the corner, and opaque in that colour where the diagonal
        // and the wave cross.
        let rgba = rasterise_rgba(state.svg(), 32).expect("drawn");
        assert_eq!(rgba[3], 0, "{state:?}: the corner");
        let middle = (16 * 32 + 16) * 4;
        let hex = format!(
            "#{:02x}{:02x}{:02x}",
            rgba[middle],
            rgba[middle + 1],
            rgba[middle + 2]
        );
        assert_eq!(
            (hex.as_str(), rgba[middle + 3]),
            (colour.as_str(), 255),
            "{state:?}"
        );
        colours.push(colour);
    }
    // The Windows tray draws "on" dark on a light taskbar by putting another colour where this
    // white is, spelled as it is spelled here.
    assert_eq!(
        colours,
        ["#6f6f6f", "#ffffff", "#2dd4e6"],
        "grey, white, cyan"
    );
    // The cyan is the application icon's.
    let app = String::from_utf8_lossy(icon::SVG).to_lowercase();
    assert!(app.contains(r##"stroke="#2dd4e6""##), "{app}");
}
