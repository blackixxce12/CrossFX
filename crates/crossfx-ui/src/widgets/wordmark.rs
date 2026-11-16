//! CrossFX's mark where the original shows its logotype.
//!
//! The original puts FxSound LLC's logotype — its five bars and its name in letters, in a box of
//! 106 × 15 points — at the left of the main window's title bar (`FxWindow.cpp:181`) and of every
//! window without a name, the message boxes (`FxWindow.cpp:272-278`). The logotype and its bars
//! are the original's, and CrossFX does not draw them. It puts its own mark in that box: the
//! wave-X of its icon, drawn as the tray draws it ([`paint_glyph`]), and its name, [`PRODUCT`] in
//! the bold face. Both are as tall as the logotype's letters stood in the box and centred on its
//! middle, the glyph at the box's left edge and the name after it, and both take the one colour
//! the caller gives, as the logotype's two halves did — at every level of «Like the original
//! FxSound LLC», since names are not parity.

use crate::theme;
use crossfx_core::identity::PRODUCT;
use egui::epaint::CubicBezierShape;
use egui::{Color32, Galley, Painter, Pos2, Rect, Shape, Stroke, Vec2, pos2};
use std::sync::Arc;

/// How tall the letters stand, as a share of the box's height: as the logotype's letters stood in
/// it (the original's `logo-white.svg`, whose `F` runs from 7.29 to 75 of its 75.15-unit
/// `viewBox`). The glyph is as tall.
pub const LETTER_HEIGHT: f32 = 0.9;

/// The room between the glyph and the name, as a share of their height.
pub const GAP: f32 = 0.4;

/// The wave-X as the tray's icons draw it (`assets/icons/status/`), in their square of 256 units:
/// the diagonal from one end to the other…
const DIAGONAL: [[f32; 2]; 2] = [[44.0, 44.0], [212.0, 212.0]];

/// …and the wave, as two cubic Béziers that meet in the middle.
const WAVE: [[[f32; 2]; 4]; 2] = [
    [
        [44.0, 212.0],
        [100.3, 212.3],
        [128.3, 184.3],
        [128.0, 128.0],
    ],
    [[128.0, 128.0], [127.7, 71.7], [155.7, 43.7], [212.0, 44.0]],
];

/// The strokes' width in those units. Their ends are round.
const STROKE: f32 = 30.0;

/// Where the glyph's ink starts on either axis, in those units: the ends' round caps reach half a
/// stroke past the ends.
const INK_FROM: f32 = 44.0 - STROKE / 2.0;

/// How far the ink runs on either axis, in those units: a square.
const INK_SPAN: f32 = 212.0 - 44.0 + STROKE;

/// How far the drawn wave may stray from the curve, as a share of the glyph's side.
const FLATTENING: f32 = 0.002;

/// Where the mark goes in its box.
#[derive(Debug, Clone)]
pub struct Mark {
    /// The square the glyph's ink fills.
    pub glyph: Rect,
    /// Where the name's galley goes, and the galley.
    pub name: (Pos2, Arc<Galley>),
}

/// The mark laid out for `rect`, or `None` for a box with no room or a face with no letters to
/// draw.
///
/// The name is set at the size whose letters are [`LETTER_HEIGHT`] of the box tall, or smaller
/// where the box is too narrow for the glyph, the [`GAP`] and the name at that size. The letters
/// are measured on the face in use, so that Manrope and a Gilroy the user added fill the box alike.
#[must_use]
pub fn layout(painter: &Painter, rect: Rect) -> Option<Mark> {
    if !rect.is_positive() {
        return None;
    }
    let laid_out = |size: f32| {
        let galley =
            painter.layout_no_wrap(PRODUCT.to_owned(), theme::bold(size), Color32::PLACEHOLDER);
        let ink = galley.mesh_bounds;
        ink.is_positive().then_some((galley, ink))
    };
    // Measured at the box's own height, close to the size it is drawn at, so that measuring costs
    // the glyph atlas no outsized letters.
    let (_, ink) = laid_out(rect.height())?;
    // The glyph is a square as tall as the letters: the whole mark is that height times one for
    // the glyph, the gap, and the name's width over its height.
    let height = (rect.height() * LETTER_HEIGHT)
        .min(rect.width() / (1.0 + GAP + ink.width() / ink.height()));
    let (galley, ink) = laid_out(rect.height() * height / ink.height())?;
    // The glyph at that height exactly: the letters' own ink may stray from it by the fraction of
    // a pixel their snapping to the pixel grid takes.
    let glyph = Rect::from_center_size(
        pos2(rect.left() + height / 2.0, rect.center().y),
        Vec2::splat(height),
    );
    let name = pos2(
        glyph.right() + height * GAP - ink.left(),
        rect.center().y - ink.center().y,
    );
    Some(Mark {
        glyph,
        name: (name, galley),
    })
}

/// Paint the mark into `rect` in `colour`.
pub fn paint(painter: &Painter, rect: Rect, colour: Color32) {
    if let Some(Mark {
        glyph,
        name: (at, galley),
    }) = layout(painter, rect)
    {
        paint_glyph(painter, glyph, colour);
        painter.galley(at, galley, colour);
    }
}

/// The wave-X in `colour`, its ink filling `square`.
pub fn paint_glyph(painter: &Painter, square: Rect, colour: Color32) {
    let scale = square.width().min(square.height()) / INK_SPAN;
    let at = |[x, y]: [f32; 2]| {
        pos2(
            square.left() + (x - INK_FROM) * scale,
            square.top() + (y - INK_FROM) * scale,
        )
    };
    let width = STROKE * scale;
    let stroke = Stroke::new(width, colour);
    painter.line_segment(DIAGONAL.map(at), stroke);
    // The wave as one line, so that its halves join without a seam.
    let mut wave: Vec<Pos2> = Vec::new();
    for half in WAVE {
        let curve =
            CubicBezierShape::from_points_stroke(half.map(at), false, Color32::TRANSPARENT, stroke);
        let points = curve.flatten(Some(FLATTENING * INK_SPAN * scale));
        // The second half starts where the first ended.
        let from = usize::from(!wave.is_empty());
        wave.extend(points.into_iter().skip(from));
    }
    painter.add(Shape::line(wave, stroke));
    // egui ends a line square; the icon's ends are round.
    for end in [DIAGONAL[0], DIAGONAL[1], WAVE[0][0], WAVE[1][3]] {
        painter.circle_filled(at(end), width / 2.0, colour);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    /// The ink of the name and the glyph's square, laid out for `rect` as the window lays them out.
    fn laid_out(rect: Rect) -> Option<(Rect, Rect)> {
        let ctx = egui::Context::default();
        ctx.set_fonts(theme::font_definitions());
        let mut mark = None;
        ctx.run_ui(Default::default(), |ui| {
            mark = layout(ui.painter(), rect).map(|mark| {
                let (at, galley) = mark.name;
                (galley.mesh_bounds.translate(at.to_vec2()), mark.glyph)
            });
        })
        .drop_without_applying_deltas();
        mark
    }

    #[test]
    fn the_glyph_and_the_name_stand_in_the_logotypes_box_as_tall_as_its_letters() {
        // The title bar's box (`Chrome::PRO.logo`) and the message boxes' (`ICON_WIDTH` ×
        // `ICON_HEIGHT`): 106 × 15.
        let rect = Rect::from_min_size(pos2(21.0, 21.0), vec2(106.0, 15.0));
        let (name, glyph) = laid_out(rect).expect("the mark is laid out");
        // A point's slack for the pixel grid the letters are snapped to.
        assert!(
            (name.height() - rect.height() * LETTER_HEIGHT).abs() <= 1.0,
            "{name:?}"
        );
        assert!((name.center().y - rect.center().y).abs() <= 1.0, "{name:?}");
        assert!(name.right() <= rect.right() + 0.5, "{name:?} in {rect:?}");
        // The glyph: a square as tall as the letters at the box's left, the gap, then the name.
        assert!((glyph.width() - glyph.height()).abs() < 1e-3, "{glyph:?}");
        assert!(
            (glyph.height() - rect.height() * LETTER_HEIGHT).abs() < 1e-3,
            "{glyph:?}"
        );
        assert!((glyph.height() - name.height()).abs() <= 1.0, "{glyph:?}");
        assert!((glyph.left() - rect.left()).abs() < 1e-3, "{glyph:?}");
        assert!(
            (glyph.center().y - rect.center().y).abs() < 1e-3,
            "{glyph:?}"
        );
        assert!(
            (name.left() - (glyph.right() + GAP * glyph.height())).abs() <= 1.0,
            "{name:?} after {glyph:?}"
        );
    }

    #[test]
    fn a_box_too_narrow_for_the_letters_height_takes_the_mark_at_its_width() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(50.0, 15.0));
        let (name, glyph) = laid_out(rect).expect("the mark is laid out");
        assert!((glyph.left() - rect.left()).abs() < 1e-3, "{glyph:?}");
        assert!((name.right() - rect.right()).abs() <= 1.0, "{name:?}");
        assert!(name.height() < rect.height() * LETTER_HEIGHT, "{name:?}");
        assert!((name.center().y - rect.center().y).abs() <= 1.0, "{name:?}");
    }

    #[test]
    fn a_box_with_no_room_draws_nothing() {
        assert!(laid_out(Rect::from_min_size(Pos2::ZERO, vec2(0.0, 15.0))).is_none());
        assert!(laid_out(Rect::NOTHING).is_none());
    }

    #[test]
    fn the_glyph_is_the_wave_x_the_tray_draws() {
        let svg =
            include_str!("../../../../assets/icons/status/io.github.blackixxce12.CrossFX-on.svg");
        let [[x0, y0], [x1, y1]] = DIAGONAL;
        let [[a, b, c, d], [_, e, f, g]] = WAVE;
        let words = |points: &[[f32; 2]]| {
            points
                .iter()
                .map(|[x, y]| format!("{x} {y}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let path = format!(
            "M{x0} {y0} L{x1} {y1} M{} C{} C{}",
            words(&[a]),
            words(&[b, c, d]),
            words(&[e, f, g])
        );
        assert!(svg.contains(&format!(r#"d="{path}""#)), "{path} in {svg}");
        assert!(svg.contains(&format!(r#"stroke-width="{STROKE}""#)));
        assert!(svg.contains(r#"stroke-linecap="round""#));
        assert!(svg.contains(r#"viewBox="0 0 256 256""#));
    }

    #[test]
    fn the_glyph_fills_its_square_and_ends_round() {
        let ctx = egui::Context::default();
        let square = Rect::from_min_size(pos2(10.0, 20.0), vec2(13.5, 13.5));
        let colour = Color32::from_rgb(0xe6, 0x34, 0x62);
        let mut output = ctx.run_ui(Default::default(), |ui| {
            paint_glyph(ui.painter(), square, colour);
        });
        let shapes = std::mem::take(&mut output.shapes);
        output.drop_without_applying_deltas();
        // The diagonal, the wave, and a round cap on each of the four ends.
        let mut bounds = Rect::NOTHING;
        let (mut lines, mut caps) = (0, 0);
        for clipped in &shapes {
            match &clipped.shape {
                Shape::LineSegment { stroke, .. } => {
                    assert_eq!(stroke.color, colour);
                    lines += 1;
                }
                Shape::Path(path) => {
                    assert!(!path.closed);
                    lines += 1;
                }
                Shape::Circle(circle) => {
                    assert_eq!(circle.fill, colour);
                    caps += 1;
                }
                other => panic!("{other:?}"),
            }
            bounds = bounds.union(clipped.shape.visual_bounding_rect());
        }
        assert_eq!((lines, caps), (2, 4));
        // The ink, caps and strokes, fills the square.
        assert!(
            (bounds.min - square.min).length() < 0.2 && (bounds.max - square.max).length() < 0.2,
            "{bounds:?} for {square:?}"
        );
    }
}
