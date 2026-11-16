//! The Lite window — 550 × 189, the two pickers and nothing else.
//!
//! Ports `FxLiteView` (`fxsound/Source/GUI/FxLiteView.cpp`, 58 lines — the whole view is a
//! `resized()` that moves two combo boxes and a `paint()` that fills one rounded rectangle). The
//! content is 550 × 112 at `(0, 57)`; the title bar above it is the same
//! [`crate::views::titlebar`] the Pro window uses, with its own `Chrome` because the bar is 508
//! points wide instead of 998.
//!
//! ```text
//! window   (0,   0, 550, 189)   rounded, radius 21, WindowBackground
//! title    (21,  0, 508,  56)   views::titlebar
//! divider  (0,  56, 550,   1)   ControlBackground
//! panel    (20, 79, 510,  90)   rounded, radius 10, DefaultFill α 0.2
//! preset   (40, 99, 225,  50)   output (285, 99, 225, 50)
//! ```
//!
//! Note what is *not* shared with Pro. The panel is `DefaultFill` at 20 % — white in the light
//! palette, black in the dark one — where Pro's is `PanelBackground`, and it is rounded by 10
//! rather than 8 (`FxLiteView.cpp:54` against `FxProView.cpp:114`). The combos keep `FxView`'s own
//! 225 × 50 size because `FxLiteView` never overrides it, which also makes them rounder (`height /
//! 5` = 10) and sets their text in 17 px rather than 14 (`docs/spec/01-window-layout.md` §5.1's
//! shadowing note).
//!
//! ## Two things the original does here that this cannot
//!
//! * **Corner snapping.** `FxMainWindow::showLiteView` teleports the window to within 10 points of
//!   whichever work-area corner the tray icon is in, every single time it is shown
//!   (`FxMainWindow.cpp:266-276`). A Wayland client cannot position itself at all; doing it
//!   properly needs `wlr-layer-shell`, which eframe does not expose
//!   (`docs/spec/01-window-layout.md` §11.1). Placement is the compositor's here.
//! * **The error notification.** `FxView::showErrorNotification` places a 560 × 120 bubble by
//!   subtracting its width from the output combo's, which in a 550 point window puts it at
//!   x = −50 and clips it (§5.1). A notice is drawn here instead as one line under the two
//!   combos, inside the panel's bottom padding ([`crate::layout::lite::notification`]), and a
//!   click on it takes it down.
//!
//! ## Two lanes in one list
//!
//! The Pro view has a device list per lane; this window keeps its one list and holds both lanes in
//! it: `Output` and `Input` titled as before, each run starting with an `Off` row that detaches
//! that lane. Picking a device makes its lane the one the preset list addresses, and the closed box
//! shows that lane's device — or `Off`, dimmed, when it has none.

use crate::assets::AssetCache;
use crate::layout;
use crate::state::{NoticeLink, UiAction, UiResponse, UiState};
use crate::theme::{FxColor, Palette};
use crate::views::{self, ViewScratch, at, titlebar, window_origin, window_rect};
use crossfx_core::ViewMode;
use egui::text::{LayoutJob, TextWrapping};
use egui::{CornerRadius, CursorIcon, Id, Pos2, Rect, Sense, Ui, pos2, vec2};

/// The notice strip's JUCE font height: the readout strip's size, a point up.
pub const NOTICE_FONT_PX: f32 = 12.0;

/// `cornerSize` of the Lite panel (`FxLiteView.cpp:54`) — ten, not the Pro view's eight.
pub const PANEL_CORNER_RADIUS: f32 = 10.0;

/// Alpha the panel is filled at (`FxLiteView.cpp:54`: `FXCOLOR(DefaultFill).withAlpha(0.2f)`).
pub const PANEL_ALPHA: f32 = 0.2;

/// Points of panel above and below the combos.
///
/// `BACKGROUND_HEIGHT = LIST_HEIGHT + 40` (`FxLiteView.h:41`), i.e. 20 on each side, and the panel
/// starts at content-local y = 22 against the combos' 42 (`FxLiteView.cpp:54`, `:37`).
pub const PANEL_PADDING: f32 = 20.0;

/// The rounded panel behind the two pickers.
///
/// `docs/spec/01-window-layout.md` §8.2 puts it at window `(20, 79, 510, 90)`: content-local
/// `(20, 22)` plus the 57 point content origin, which leaves exactly [`PANEL_PADDING`] above and
/// below the 50 point combos at y = 99. It is derived from [`crate::layout::lite::preset_combo`]
/// here rather than read from [`crate::layout::lite::panel`] because that function currently
/// returns y = 73 — the *Pro* panel's y — which would sit the panel six points high of the combos
/// it is supposed to frame. Deriving it keeps the two from drifting apart whichever way that is
/// settled.
#[must_use]
pub fn panel(origin: Pos2) -> Rect {
    let combos = layout::lite::preset_combo().union(layout::lite::output_combo());
    let reference = layout::lite::panel();
    at(
        origin,
        Rect::from_min_size(
            pos2(reference.left(), combos.top() - PANEL_PADDING),
            vec2(reference.width(), combos.height() + PANEL_PADDING * 2.0),
        ),
    )
}

/// Paint the Lite window and report what the user did.
///
/// First the window's face is brought up to date, as the Pro window's is ([`super::pro::show`]).
pub fn show(
    ui: &mut Ui,
    state: &UiState,
    scratch: &mut ViewScratch,
    palette: Palette,
    assets: &mut AssetCache,
) -> UiResponse {
    crate::typeface::sync(ui.ctx(), palette.is_windows());
    let origin = window_origin(ui);

    ui.painter().rect_filled(
        window_rect(origin, ViewMode::Lite),
        CornerRadius::same(layout::WINDOW_CORNER_RADIUS as u8),
        palette.window_background(),
    );
    ui.painter().rect_filled(
        panel(origin),
        CornerRadius::same(PANEL_CORNER_RADIUS as u8),
        palette.color_alpha(FxColor::DefaultFill, PANEL_ALPHA),
    );

    let mut response = titlebar::show(ui, state, scratch, palette, assets);

    views::preset_combo(
        ui,
        state,
        palette,
        assets,
        at(origin, layout::lite::preset_combo()),
        &mut response,
    );
    views::device_combo(
        ui,
        state,
        palette,
        assets,
        at(origin, layout::lite::output_combo()),
        &mut response,
    );

    if let Some(text) = &state.notification {
        notice_strip(
            ui,
            text,
            state.notice_link,
            palette,
            at(origin, layout::lite::notification()),
            &mut response,
        );
    }

    response
}

/// The room between the notice and its link's button in the strip.
pub const NOTICE_LINK_GAP: f32 = 12.0;

/// The notice as one line: centred under the combos, with its link's button after it when it
/// offers one ([`NoticeLink`]). The line is set at [`NOTICE_FONT_PX`], or smaller down to
/// [`crate::dialogs::MIN_FIT_SCALE`] of it to fit the strip, and only then is the text elided —
/// never the button. The whole notice, every line of it and the link's note, is the strip's
/// tooltip, and the notice stays up while the pointer is on it ([`UiAction::HoldNotice`]): a
/// sentence too long for the strip is read there, and the link reached before it goes. Returns
/// where the text went.
fn notice_strip(
    ui: &Ui,
    text: &str,
    link: Option<NoticeLink>,
    palette: Palette,
    strip: Rect,
    response: &mut UiResponse,
) -> Rect {
    let colour = palette.color(FxColor::DefaultText);
    let label = link.map(NoticeLink::label);
    let line = text.lines().next().unwrap_or_default();
    // A point short of the strip: a line elided at a fitted size can overshoot by a fraction.
    let room = strip.width() - 1.0;
    let font = notice_strip_font(ui.ctx(), line, label.as_deref(), room);
    let label_galley = label.as_ref().map(|label| {
        ui.painter().layout_no_wrap(
            label.clone(),
            font.clone(),
            palette.color(FxColor::HighlightedText),
        )
    });
    let label_room = label_galley
        .as_ref()
        .map_or(0.0, |galley| galley.size().x + NOTICE_LINK_GAP);
    // One line only: the first line of a multi-line message, and that elided to what is left.
    let mut job =
        LayoutJob::single_section(line.to_owned(), egui::TextFormat::simple(font, colour));
    job.wrap = TextWrapping::truncate_at_width((room - label_room).max(0.0));
    let galley = ui.painter().layout_job(job);
    let width = galley.size().x + label_room;
    let left = strip.center().x - width / 2.0;
    let placed = Rect::from_min_size(
        pos2(left, strip.center().y - galley.size().y / 2.0),
        galley.size(),
    );
    ui.painter().galley(placed.min, galley, colour);

    // The strip first and the button over it: the later of two is the one a click on both
    // reaches.
    let whole = link.map_or_else(
        || text.to_owned(),
        |link| format!("{text}\n\n{}", link.note()),
    );
    let hit = ui.interact(strip, Id::unique("fx_notice_strip"), Sense::click());
    if hit.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        response.push(UiAction::HoldNotice);
        // Frames keep coming while it is held, so that the hold outlasts a still pointer.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(1));
    }
    if hit.clicked() {
        response.push(UiAction::DismissNotice);
    }
    let _ = hit.on_hover_text(whole);

    if let (Some(link), Some(label_galley)) = (link, label_galley) {
        let size = label_galley.size();
        let button = Rect::from_min_size(
            pos2(
                placed.right() + NOTICE_LINK_GAP,
                strip.center().y - size.y / 2.0,
            ),
            size,
        );
        let link_colour = palette.color(FxColor::HighlightedText);
        ui.painter().galley(button.min, label_galley, link_colour);
        ui.painter().hline(
            button.x_range(),
            button.bottom() - 1.0,
            egui::Stroke::new(1.0, link_colour),
        );
        let hit = ui.interact(button, Id::unique("fx_notice_link"), Sense::click());
        if hit.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            response.push(UiAction::HoldNotice);
        }
        if hit.clicked() {
            response.push(UiAction::OpenNoticeLink(link));
        }
        let _ = hit.on_hover_text(link.note());
    }

    placed
}

/// The font the strip's line is set in: [`NOTICE_FONT_PX`]'s caption font, or the size down to
/// [`crate::dialogs::MIN_FIT_SCALE`] of it at which `line` fits `room` — side by side with the
/// link's `label`, when there is one.
#[must_use]
pub fn notice_strip_font(
    ctx: &egui::Context,
    line: &str,
    label: Option<&str>,
    room: f32,
) -> egui::FontId {
    let font = views::pro::caption_font(NOTICE_FONT_PX);
    let Some(label) = label else {
        return crate::dialogs::fitted_font(ctx, line, font, room);
    };
    let label_width = |font: &egui::FontId| {
        ctx.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap(label.to_owned(), font.clone(), egui::Color32::PLACEHOLDER)
                .size()
                .x
        })
    };
    // The label shrinks with the line: fit the two as one, then check the label's own share.
    let joined = format!("{line}{label}");
    let mut fitted =
        crate::dialogs::fitted_font(ctx, &joined, font.clone(), room - NOTICE_LINK_GAP);
    while fitted.size > font.size * crate::dialogs::MIN_FIT_SCALE
        && notice_line_width(ctx, line, &fitted) + label_width(&fitted) + NOTICE_LINK_GAP > room
    {
        fitted.size -= 0.5;
    }
    fitted
}

/// How wide `line` is set in `font`.
fn notice_line_width(ctx: &egui::Context, line: &str, font: &egui::FontId) -> f32 {
    ctx.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(line.to_owned(), font.clone(), egui::Color32::PLACEHOLDER)
            .size()
            .x
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::PresetEntry;
    use crate::theme;
    use crate::views::testing::{Harness, painted_bounds, text_below, texts};
    use crate::widgets::combo;
    use crossfx_core::{AudioDevice, DeviceDirection, ThemeMode};
    use egui::{Event, RawInput};

    fn state() -> UiState {
        UiState {
            view: ViewMode::Lite,
            presets: vec![
                PresetEntry {
                    name: "Flat".to_owned(),
                    factory: true,
                    modified: false,
                },
                PresetEntry {
                    name: "My Mix".to_owned(),
                    factory: false,
                    modified: false,
                },
            ],
            selected_preset: Some(1),
            devices: vec![AudioDevice {
                id: 7,
                name: "alsa_output.usb-Focusrite".to_owned(),
                description: "Scarlett 2i2 Analogue Stereo".to_owned(),
                is_default: false,
                direction: crossfx_core::DeviceDirection::Output,
                form_factor: "speaker".into(),
                is_virtual: false,
            }],
            selected_output: Some(0),
            ..UiState::default()
        }
    }

    fn test_context() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(theme::font_definitions());
        ctx
    }

    fn frame(
        ctx: &egui::Context,
        state: &UiState,
        scratch: &mut ViewScratch,
        assets: &mut AssetCache,
        events: Vec<Event>,
    ) -> Vec<UiAction> {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(
                Pos2::ZERO,
                views::window_size(ViewMode::Lite),
            )),
            events,
            ..Default::default()
        };
        let mut actions = Vec::new();
        ctx.run_ui(input, |ui| {
            actions = show(ui, state, scratch, Palette::new(ThemeMode::Dark), assets).actions;
        })
        .drop_without_applying_deltas();
        actions
    }

    #[test]
    fn the_panel_is_the_one_the_spec_tabulates() {
        // docs/spec/01-window-layout.md §8.2: (20, 79, 510, 90).
        let panel = panel(Pos2::ZERO);
        assert!((panel.left() - 20.0).abs() < 1e-4, "{panel:?}");
        assert!((panel.top() - 79.0).abs() < 1e-4, "{panel:?}");
        assert!((panel.width() - 510.0).abs() < 1e-4, "{panel:?}");
        assert!((panel.height() - 90.0).abs() < 1e-4, "{panel:?}");
    }

    #[test]
    fn the_panel_frames_the_combos_with_twenty_points_on_every_side() {
        let panel = panel(Pos2::ZERO);
        let preset = layout::lite::preset_combo();
        let output = layout::lite::output_combo();
        assert!(panel.contains_rect(preset), "{preset:?} escapes {panel:?}");
        assert!(panel.contains_rect(output), "{output:?} escapes {panel:?}");
        assert!((preset.top() - panel.top() - PANEL_PADDING).abs() < 1e-4);
        assert!((panel.bottom() - preset.bottom() - PANEL_PADDING).abs() < 1e-4);
        assert!((preset.left() - panel.left() - PANEL_PADDING).abs() < 1e-4);
        assert!((panel.right() - output.right() - PANEL_PADDING).abs() < 1e-4);
        // …and a 20 point gutter between them, which is where BACKGROUND_WIDTH's third 20 goes.
        assert!((output.left() - preset.right() - PANEL_PADDING).abs() < 1e-4);
    }

    #[test]
    fn the_panel_sits_inside_the_window_with_twenty_points_of_margin() {
        let window = window_rect(Pos2::ZERO, ViewMode::Lite);
        let panel = panel(Pos2::ZERO);
        assert!(window.contains_rect(panel));
        assert!((panel.left() - window.left() - 20.0).abs() < 1e-4);
        assert!((window.right() - panel.right() - 20.0).abs() < 1e-4);
        // 20 points of dead space under the panel, the rounded bottom corners' room.
        assert!((window.bottom() - panel.bottom() - 20.0).abs() < 1e-4);
    }

    #[test]
    fn the_lite_combos_are_rounder_and_set_larger_than_the_pro_ones() {
        // FxLiteView never overrides FxView's 225 x 50, so `height / 5` is 10 rather than 8 and
        // the font stays at 17 instead of dropping to 14 (docs/spec/01-window-layout.md §5.1).
        let lite = layout::lite::preset_combo();
        let pro = layout::pro::preset_combo();
        assert!((combo::corner_radius(lite.height()) - 10.0).abs() < 1e-4);
        assert!((combo::corner_radius(pro.height()) - 8.0).abs() < 1e-4);
        assert!((combo::font_size(lite.height()) - combo::FONT).abs() < 1e-4);
        assert!((lite.width() - 225.0).abs() < 1e-4);
        assert!((lite.height() - 50.0).abs() < 1e-4);
    }

    #[test]
    fn a_quiet_lite_frame_reports_nothing() {
        let ctx = test_context();
        let mut scratch = ViewScratch::new();
        let mut assets = AssetCache::new();
        let state = state();
        for _ in 0..2 {
            let actions = frame(&ctx, &state, &mut scratch, &mut assets, Vec::new());
            assert!(actions.is_empty(), "an idle window reported {actions:?}");
        }
    }

    #[test]
    fn nothing_the_window_paints_escapes_the_window() {
        // The Lite window is 490 points shorter and 508 narrower than the Pro one, so anything
        // that kept a Pro coordinate by accident would hang off it — the notice included, which in
        // the original started 50 points left of the window.
        let window = window_rect(Pos2::ZERO, ViewMode::Lite);
        let long = "A notice far longer than the two combos are wide, which has to be cut short \
                    rather than run past the panel, the window, or anything else in its way.";
        for (label, state) in [
            ("plain", state()),
            (
                "with a notice",
                UiState {
                    notification: Some(long.to_owned()),
                    ..state()
                },
            ),
            (
                "editing the microphone",
                UiState {
                    notification: Some("Preset: Clean Voice".to_owned()),
                    ..lanes()
                },
            ),
        ] {
            for mode in [ThemeMode::Dark, ThemeMode::Light] {
                let mut harness = Harness::new(mode);
                let shapes = harness.settle(&state);
                let painted = painted_bounds(&shapes);
                assert!(painted.is_positive(), "{label}: the window painted nothing");
                assert!(
                    window.expand(1.0).contains_rect(painted),
                    "{label}, {mode:?}: {painted:?} spills out of {window:?}"
                );
                assert!(
                    painted.width() > window.width() - 2.0
                        && painted.height() > window.height() - 2.0,
                    "{label}: only {painted:?} of {window:?} was painted"
                );
            }
        }
    }

    #[test]
    fn the_lite_window_still_carries_the_whole_title_bar() {
        // The flip button is the one chrome button whose position differs between the views, so
        // clicking Lite's proves the bar was laid out with `Chrome::LITE`.
        let ctx = test_context();
        let mut scratch = ViewScratch::new();
        let mut assets = AssetCache::new();
        let state = state();
        let target = crate::layout::Chrome::LITE.flip.rect().center();

        let mut actions = Vec::new();
        for events in [
            vec![Event::PointerMoved(target)],
            vec![
                Event::PointerMoved(target),
                Event::PointerButton {
                    pos: target,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
            ],
            vec![Event::PointerButton {
                pos: target,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            }],
        ] {
            actions.extend(frame(&ctx, &state, &mut scratch, &mut assets, events));
        }
        assert_eq!(actions, vec![UiAction::ToggleView]);
    }

    #[test]
    fn with_the_power_off_the_preset_list_and_the_output_list_both_still_open() {
        // FxLiteView.cpp:57 gates `preset_list_` with the power and leaves the output list
        // alone. CrossFX leaves both live (0.4.0 audit R7): the command line picks a preset with
        // the power off, and so can the window.
        let state = UiState {
            power: false,
            ..state()
        };
        for (combo, salt) in [
            (layout::lite::preset_combo(), "preset_list"),
            (layout::lite::output_combo(), "output_list"),
        ] {
            let ctx = test_context();
            let mut scratch = ViewScratch::new();
            let mut assets = AssetCache::new();
            let at = combo.center();
            frame(
                &ctx,
                &state,
                &mut scratch,
                &mut assets,
                vec![Event::PointerMoved(at)],
            );
            frame(
                &ctx,
                &state,
                &mut scratch,
                &mut assets,
                vec![
                    Event::PointerMoved(at),
                    Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::default(),
                    },
                    Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::default(),
                    },
                ],
            );
            // `FxComboBox` hangs its menu off `Id::unique("fx_combo_box").with(id_salt)`, and
            // `Popup::default_response_id` is that id with "popup" mixed in.
            let popup_id = egui::Id::unique("fx_combo_box").with(salt).with("popup");
            assert!(
                egui::Popup::is_id_open(&ctx, popup_id),
                "{salt} should open with the power off"
            );
        }
    }

    /// Whether a click on the Lite window's `combo`, its menu hung off `salt`, opens the menu.
    fn click_opens(state: &UiState, combo: egui::Rect, salt: &str) -> bool {
        let ctx = test_context();
        let mut scratch = ViewScratch::new();
        let mut assets = AssetCache::new();
        let at = combo.center();
        let button = |pressed| Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        frame(
            &ctx,
            state,
            &mut scratch,
            &mut assets,
            vec![Event::PointerMoved(at)],
        );
        frame(
            &ctx,
            state,
            &mut scratch,
            &mut assets,
            vec![Event::PointerMoved(at), button(true), button(false)],
        );
        egui::Popup::is_id_open(
            &ctx,
            egui::Id::unique("fx_combo_box").with(salt).with("popup"),
        )
    }

    #[test]
    fn at_interface_with_the_power_off_the_preset_list_stays_shut_as_on_windows() {
        // FxLiteView.cpp:57 at «Like the original FxSound LLC» = Interface (0.4.0 audit R7 set
        // back): the preset list goes with the power, the output list does not.
        let off = UiState {
            power: false,
            windows_parity: crossfx_core::WindowsParity::Interface,
            ..state()
        };
        assert!(!click_opens(
            &off,
            layout::lite::preset_combo(),
            "preset_list"
        ));
        assert!(click_opens(
            &off,
            layout::lite::output_combo(),
            "output_list"
        ));
        let on = UiState { power: true, ..off };
        assert!(click_opens(
            &on,
            layout::lite::preset_combo(),
            "preset_list"
        ));
    }

    // ---- two lanes in one list ---------------------------------------------------------------

    fn device(name: &str, direction: DeviceDirection) -> AudioDevice {
        AudioDevice {
            id: 0,
            name: format!("node.{name}"),
            description: name.to_owned(),
            is_default: false,
            direction,
            form_factor: String::new(),
            is_virtual: false,
        }
    }

    /// A speaker and a microphone, both lanes on, the microphone being edited.
    fn lanes() -> UiState {
        UiState {
            devices: vec![
                device("Speakers", DeviceDirection::Output),
                device("Microphone", DeviceDirection::Input),
            ],
            selected_output: Some(0),
            selected_input: Some(1),
            direction: DeviceDirection::Input,
            ..state()
        }
    }

    /// Open the device list and return what its open menu painted.
    ///
    /// On a screen tall enough for the whole menu: in the 189-point window it scrolls, and which
    /// rows are in view is egui's business, not this list's.
    fn open(harness: &mut Harness, state: &UiState) -> Vec<egui::epaint::ClippedShape> {
        harness.screen = Some(egui::vec2(550.0, 700.0));
        harness.click(state, layout::lite::output_combo().center());
        harness.frame(state, Vec::new());
        harness.frame(state, Vec::new()).1
    }

    #[test]
    fn the_list_titles_both_lanes_with_an_off_row_under_each_title() {
        let mut harness = Harness::new(ThemeMode::Dark);
        let state = lanes();
        let shapes = open(&mut harness, &state);
        let below = layout::lite::output_combo().bottom() - 1.0;
        let output = text_below(&shapes, "Output", below);
        let input = text_below(&shapes, "Input", below);
        let offs = text_below(&shapes, "Off", below);
        let speakers = text_below(&shapes, "Speakers", below);
        let microphone = text_below(&shapes, "Microphone", below);
        assert_eq!((output.len(), input.len()), (1, 1));
        assert_eq!(offs.len(), 2, "one Off per lane");
        // Output, Off, Speakers, Input, Off, Microphone — top to bottom.
        let order = [
            output[0],
            offs[0],
            speakers[0],
            input[0],
            offs[1],
            microphone[0],
        ];
        for pair in order.windows(2) {
            assert!(
                pair[0].top() < pair[1].top(),
                "{:?} is not above {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn picking_off_under_input_detaches_only_the_input() {
        let mut harness = Harness::new(ThemeMode::Dark);
        let state = lanes();
        let shapes = open(&mut harness, &state);
        let below = layout::lite::output_combo().bottom() - 1.0;
        let input = text_below(&shapes, "Input", below)[0];
        let off = text_below(&shapes, "Off", below)
            .into_iter()
            .find(|rect| rect.top() > input.top())
            .expect("an Off under Input");
        assert_eq!(
            harness.click(&state, off.center()),
            vec![UiAction::DetachInput]
        );
    }

    #[test]
    fn picking_a_speaker_while_editing_the_microphone_edits_the_output() {
        let mut harness = Harness::new(ThemeMode::Dark);
        let state = lanes();
        let shapes = open(&mut harness, &state);
        let below = layout::lite::output_combo().bottom() - 1.0;
        let speakers = text_below(&shapes, "Speakers", below)[0];
        assert_eq!(
            harness.click(&state, speakers.center()),
            vec![
                UiAction::SetEditDirection(DeviceDirection::Output),
                UiAction::SelectOutput(0)
            ]
        );
    }

    #[test]
    fn opening_the_single_list_does_not_move_the_edit_direction() {
        // It holds both lanes, so a click on it says nothing about which one is wanted.
        let mut harness = Harness::new(ThemeMode::Dark);
        let actions = harness.click(&lanes(), layout::lite::output_combo().center());
        assert!(actions.is_empty(), "{actions:?}");
    }

    #[test]
    fn the_closed_box_shows_the_edit_directions_device_or_off() {
        let combo = layout::lite::output_combo();
        let shown = |state: &UiState| {
            let mut harness = Harness::new(ThemeMode::Dark);
            let shapes = harness.settle(state);
            texts(&shapes)
                .into_iter()
                .filter(|(_, rect, _)| combo.contains(rect.center()))
                .map(|(text, _, _)| text)
                .collect::<Vec<_>>()
        };
        assert_eq!(shown(&lanes()), ["Microphone"]);
        let output = UiState {
            direction: DeviceDirection::Output,
            ..lanes()
        };
        assert_eq!(shown(&output), ["Speakers"]);
        let detached = UiState {
            selected_input: None,
            ..lanes()
        };
        assert_eq!(shown(&detached), ["Off"]);
    }

    // ---- the notice strip --------------------------------------------------------------------

    #[test]
    fn a_notice_is_one_line_under_the_combos_inside_the_panel() {
        let mut harness = Harness::new(ThemeMode::Dark);
        let state = UiState {
            notification: Some("Preset: Rock".to_owned()),
            ..state()
        };
        let shapes = harness.settle(&state);
        let line = texts(&shapes)
            .into_iter()
            .find(|(text, _, _)| text == "Preset: Rock")
            .expect("the notice is painted");
        let strip = layout::lite::notification();
        assert!(
            strip.contains_rect(line.1),
            "{:?} is outside {strip:?}",
            line.1
        );
        assert!(panel(Pos2::ZERO).contains_rect(line.1));
        assert!(
            (line.1.center().x - strip.center().x).abs() < 1.0,
            "centred"
        );
        assert!(line.1.top() > layout::lite::output_combo().bottom());
    }

    #[test]
    fn no_notice_draws_nothing_under_the_combos() {
        let mut harness = Harness::new(ThemeMode::Dark);
        let shapes = harness.settle(&state());
        let strip = layout::lite::notification();
        assert!(
            texts(&shapes)
                .iter()
                .all(|(_, rect, _)| !strip.contains(rect.center())),
            "text under the combos with no notice"
        );
    }

    #[test]
    fn a_long_or_multi_line_notice_is_cut_to_one_line_that_fits() {
        let mut harness = Harness::new(ThemeMode::Light);
        let text = format!("{}\nsecond line", "word ".repeat(80));
        let state = UiState {
            notification: Some(text),
            ..state()
        };
        let shapes = harness.settle(&state);
        let line = texts(&shapes)
            .into_iter()
            .find(|(text, _, _)| text.starts_with("word"))
            .expect("the notice is painted");
        assert!(!line.0.contains("second line"), "one line only");
        let strip = layout::lite::notification();
        assert!(
            strip.expand(0.5).contains_rect(line.1),
            "{:?} is outside {strip:?}",
            line.1
        );
    }

    #[test]
    fn clicking_the_notice_takes_it_down() {
        let mut harness = Harness::new(ThemeMode::Dark);
        let state = UiState {
            notification: Some("Preset: Rock".to_owned()),
            ..state()
        };
        harness.settle(&state);
        let actions = harness.click(&state, layout::lite::notification().center());
        assert!(
            actions.contains(&UiAction::HoldNotice),
            "held while the pointer is on it"
        );
        let others: Vec<_> = actions
            .into_iter()
            .filter(|action| *action != UiAction::HoldNotice)
            .collect();
        assert_eq!(others, vec![UiAction::DismissNotice]);
    }

    /// Every line of text painted with the pointer resting on `pos`, tooltips shown at once.
    fn hovered(harness: &mut Harness, state: &UiState, pos: Pos2) -> Vec<String> {
        harness.ctx.all_styles_mut(|style| {
            style.interaction.tooltip_delay = 0.0;
            style.interaction.show_tooltips_only_when_still = false;
        });
        harness.frame(state, vec![Event::PointerMoved(pos)]);
        harness.frame(state, Vec::new());
        let (_, shapes) = harness.frame(state, Vec::new());
        texts(&shapes).into_iter().map(|(text, ..)| text).collect()
    }
    #[test]
    fn a_notice_cut_to_the_strip_shows_all_of_itself_on_hover() {
        // The strip has room for one line of about seventy letters; the rest of a longer notice,
        // and every line after its first, was nowhere to be read (v3-lite-notice-cuts-the-remedy).
        let mut harness = Harness::new(ThemeMode::Dark);
        let text = format!(
            "{}the remedy at its end
and a second line",
            "a long cause, ".repeat(8)
        );
        let state = UiState {
            notification: Some(text.clone()),
            ..state()
        };
        let shapes = harness.settle(&state);
        let painted = texts(&shapes)
            .into_iter()
            .find(|(line, _, _)| line.starts_with("a long cause"))
            .expect("the notice is painted");
        let strip = layout::lite::notification();
        assert!(
            strip.expand(0.5).contains_rect(painted.1),
            "cut to the strip: {:?}",
            painted.1
        );
        let shown = hovered(&mut harness, &state, layout::lite::notification().center());
        assert!(
            shown.contains(&text),
            "the whole notice on hover: {shown:?}"
        );
    }

    #[test]
    fn a_notice_with_a_link_has_its_button_after_the_text_and_the_button_opens_the_page() {
        let mut harness = Harness::new(ThemeMode::Dark);
        let state = UiState {
            notification: Some("No virtual playback device: install VB-CABLE".to_owned()),
            notice_link: Some(NoticeLink::VbCable),
            ..state()
        };
        let shapes = harness.settle(&state);
        let painted = texts(&shapes);
        let find = |wanted: &str| {
            painted
                .iter()
                .find(|(text, _, _)| text == wanted)
                .map(|(_, rect, _)| *rect)
                .unwrap_or_else(|| panic!("{wanted:?} is painted"))
        };
        let notice = find("No virtual playback device: install VB-CABLE");
        let button = find(crate::state::GET_VB_CABLE);
        let strip = layout::lite::notification();
        assert!(strip.expand(0.5).contains_rect(notice));
        assert!(
            strip.expand(0.5).contains_rect(button),
            "{button:?} in {strip:?}"
        );
        assert!(button.left() >= notice.right() + NOTICE_LINK_GAP - 0.5);
        // The note goes with the button, on hover.
        let shown = hovered(&mut harness, &state, button.center());
        assert!(
            shown
                .iter()
                .any(|line| line == crate::state::VB_CABLE_EXPLAINED),
            "{shown:?}"
        );
        let actions = harness.click(&state, button.center());
        assert!(
            actions.contains(&UiAction::OpenNoticeLink(NoticeLink::VbCable)),
            "{actions:?}"
        );
        assert!(!actions.contains(&UiAction::DismissNotice), "{actions:?}");
    }
}
