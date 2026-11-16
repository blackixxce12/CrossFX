//! CrossFX's name where the original shows its logotype.
//!
//! The original puts FxSound LLC's logotype — its five bars and its name in letters, in a box of
//! 106 × 15 points — at the left of the main window's title bar (`FxWindow.cpp:181`) and of every
//! window without a name, the message boxes (`FxWindow.cpp:272-278`). The logotype and its bars
//! are the original's, and CrossFX does not draw them: it names itself in that box, with the text
//! [`PRODUCT`] in the bold face, its letters as tall as the logotype's stood in the box, from the
//! box's left edge and centred on its middle — at every level of «Like the original FxSound LLC»,
//! since names are not parity. An icon of CrossFX's own, once it has one, goes in front of the
//! name.

use crate::theme;
use egui::{Color32, Galley, Painter, Pos2, Rect, pos2};
use crossfx_core::identity::PRODUCT;
use std::sync::Arc;

/// How tall the letters stand, as a share of the box's height: as the logotype's letters stood in
/// it (`logo-white.svg`, whose `F` runs from 7.29 to 75 of its 75.15-unit `viewBox`).
pub const LETTER_HEIGHT: f32 = 0.9;

/// The name laid out for `rect`, and where its galley goes, or `None` for a box with no room or a
/// face with no letters to draw.
///
/// It is set at the size whose letters are [`LETTER_HEIGHT`] of the box tall, or smaller where the
/// box is too narrow for the name at that size. The letters are measured on the face in use, so
/// that Manrope and a Gilroy the user added fill the box alike.
#[must_use]
pub fn layout(painter: &Painter, rect: Rect) -> Option<(Pos2, Arc<Galley>)> {
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
    let scale = (rect.height() * LETTER_HEIGHT / ink.height()).min(rect.width() / ink.width());
    let (galley, ink) = laid_out(rect.height() * scale)?;
    Some((
        pos2(rect.left() - ink.left(), rect.center().y - ink.center().y),
        galley,
    ))
}

/// Paint the name into `rect` in `colour`.
pub fn paint(painter: &Painter, rect: Rect, colour: Color32) {
    if let Some((pos, galley)) = layout(painter, rect) {
        painter.galley(pos, galley, colour);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    /// The ink of the name painted into `rect`, measured as the window measures it.
    fn ink(rect: Rect) -> Option<Rect> {
        let ctx = egui::Context::default();
        ctx.set_fonts(theme::font_definitions());
        let mut ink = None;
        ctx.run_ui(Default::default(), |ui| {
            ink = layout(ui.painter(), rect)
                .map(|(pos, galley)| galley.mesh_bounds.translate(pos.to_vec2()));
        })
        .drop_without_applying_deltas();
        ink
    }

    #[test]
    fn the_name_stands_in_the_logotypes_box_as_tall_as_its_letters_from_the_left() {
        // The title bar's box (`Chrome::PRO.logo`) and the message boxes' (`ICON_WIDTH` ×
        // `ICON_HEIGHT`): 106 × 15.
        let rect = Rect::from_min_size(pos2(21.0, 21.0), vec2(106.0, 15.0));
        let ink = ink(rect).expect("the name is drawn");
        // A point's slack for the pixel grid the glyphs are snapped to.
        assert!((ink.left() - rect.left()).abs() <= 1.0, "{ink:?}");
        assert!((ink.center().y - rect.center().y).abs() <= 1.0, "{ink:?}");
        assert!(
            (ink.height() - rect.height() * LETTER_HEIGHT).abs() <= 1.0,
            "{ink:?}"
        );
        assert!(ink.right() <= rect.right(), "{ink:?}");
    }

    #[test]
    fn a_box_too_narrow_for_the_letters_height_takes_the_name_at_its_width() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(40.0, 15.0));
        let ink = ink(rect).expect("the name is drawn");
        assert!((ink.width() - rect.width()).abs() <= 1.0, "{ink:?}");
        assert!(ink.height() < rect.height() * LETTER_HEIGHT, "{ink:?}");
        assert!((ink.center().y - rect.center().y).abs() <= 1.0, "{ink:?}");
    }

    #[test]
    fn a_box_with_no_room_draws_nothing() {
        assert_eq!(ink(Rect::from_min_size(Pos2::ZERO, vec2(0.0, 15.0))), None);
        assert_eq!(ink(Rect::NOTHING), None);
    }
}
