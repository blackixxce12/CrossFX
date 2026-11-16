//! CrossFX's icon, "wave-X": a straight white diagonal crossed by a cyan wave on a dark tile.
//!
//! It is two drawings in `assets/icons/apps/`: `crossfx.svg`, the master, for 48 pixels and up and
//! as the scalable icon the Linux packages install, and `crossfx-small.svg`, the same drawing with
//! heavier strokes on a larger tile for 16 to 32 pixels, where the master's strokes would thin to
//! a blur. Every bitmap of the icon is the drawing meant for its size drawn at that size, never
//! another size resampled: the PNGs beside the drawings, which the packages install under
//! `hicolor/<size>x<size>/apps`, every frame of `crossfx.exe`'s icon (`assets/windows/crossfx.ico`)
//! and the window's icon on Windows. `examples/icons.rs` draws the files, and `tests/icons.rs`
//! holds the ones in the tree to what [`pixmap`] draws.
//!
//! The tray's three icons are the same wave-X without the tile, one colour per state
//! (`assets/icons/status/`, [`crate::tray`]).

/// The master drawing, for every size above [`SMALL_UP_TO`].
pub const SVG: &[u8] = include_bytes!("../../../assets/icons/apps/crossfx.svg");

/// The cut for small sizes, up to [`SMALL_UP_TO`].
pub const SMALL_SVG: &[u8] = include_bytes!("../../../assets/icons/apps/crossfx-small.svg");

/// The largest size, in pixels, drawn from [`SMALL_SVG`].
pub const SMALL_UP_TO: u32 = 32;

/// Every size of the icon the tree carries as a PNG, `assets/icons/apps/crossfx-<size>.png`, and
/// every package installs as `hicolor/<size>x<size>/apps/io.github.blackixxce12.CrossFX.png`.
pub const SIZES: [u32; 8] = [16, 24, 32, 48, 64, 128, 256, 512];

/// The frames of `crossfx.exe`'s icon, `assets/windows/crossfx.ico`, the sizes Windows asks for:
/// 16, 24 and 32 for the title bar, the taskbar and the Start menu at 100-200 % scaling, 48 and 64
/// for Explorer's medium icons, 128 and 256 for its large and extra-large views.
pub const ICO_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

/// The frames of the `.ico` from this size up are PNG-compressed, as Windows has taken them since
/// Vista: a 256-pixel frame as a bitmap would weigh a quarter of a megabyte. The smaller ones are
/// 32-bit bitmaps with their AND masks, which every icon reader understands.
pub const ICO_PNG_FROM: u32 = 256;

/// The drawing for an icon of `size` pixels.
#[must_use]
pub const fn svg_for(size: u32) -> &'static [u8] {
    if size <= SMALL_UP_TO { SMALL_SVG } else { SVG }
}

/// The icon at `size` × `size` pixels, premultiplied as tiny-skia keeps it. `None` for a size of
/// zero.
#[must_use]
pub fn pixmap(size: u32) -> Option<tiny_skia::Pixmap> {
    crate::tray::render(svg_for(size), size)
}

/// The icon at `size` × `size` pixels as straight-alpha RGBA, what a window icon is made of.
/// `None` for a size of zero.
#[must_use]
pub fn rgba(size: u32) -> Option<Vec<u8>> {
    Some(pixmap(size)?.take_demultiplied())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_small_cut_draws_up_to_thirty_two_pixels_and_the_master_above() {
        assert_ne!(SMALL_SVG, SVG);
        for size in SIZES {
            let expected = if size <= 32 { SMALL_SVG } else { SVG };
            assert!(svg_for(size) == expected, "{size}");
        }
        // Every frame of the .ico is a size the PNGs have too.
        assert!(ICO_SIZES.iter().all(|size| SIZES.contains(size)));
        assert!(ICO_SIZES.contains(&ICO_PNG_FROM));
    }

    #[test]
    fn the_icon_is_a_tile_with_clear_corners_at_every_size() {
        for size in SIZES {
            let rgba = rgba(size).expect("the icon is drawn");
            assert_eq!(rgba.len(), (size * size * 4) as usize, "{size}");
            let alpha = |x: u32, y: u32| rgba[((y * size + x) * 4 + 3) as usize];
            // The tile's rounded corners leave the corners of the square clear, and its middle,
            // where the diagonal and the wave cross, is opaque.
            for (x, y) in [(0, 0), (size - 1, 0), (0, size - 1), (size - 1, size - 1)] {
                assert_eq!(alpha(x, y), 0, "{size}: ({x}, {y})");
            }
            assert_eq!(alpha(size / 2, size / 2), 255, "{size}: the middle");
        }
        assert_eq!(pixmap(0).map(|p| p.width()), None);
    }
}
