//! Every line of text the windows draw fits the box it is drawn in: in Manrope, in every language.
//!
//! The window's boxes were measured in Gilroy (`docs/spec/01-window-layout.md` §2.3), and the
//! font that fills them now is Manrope at its type scale (`crossfx_ui::typeface`). The scale makes
//! Manrope's text narrower than Gilroy's on the whole — 0.95 of it in the Latin and Cyrillic
//! tables — but not word by word: a short word full of `t`s and `f`s, French "Effets" or Czech
//! "Zrušit", comes out up to 8 % wider. So no box is taken on trust. Each window is drawn
//! headless, as the application draws it, in each of the thirty languages, and every text it
//! paints is held to four rules:
//!
//! - **not cut**: a line elided with `…` ([`egui::Galley::elided`]) is a label its box could not
//!   hold;
//! - **not clipped**: a wrapped text whose lines run past the rectangle it is clipped to is a
//!   note its box could not hold;
//! - **not over another text**: a label that spills into its neighbour's room;
//! - **inside the window**.
//!
//! What the design cuts on purpose is in [`designed_cut`], each with its reason. The library's
//! own tests measure many of these boxes one by one, key by key; this one draws the windows whole,
//! so a box nobody thought to measure is measured too.
//!
//! The language is the process's (`crossfx_core::i18n::set_language`), which is why this is a
//! test binary of its own, its tests taking turns at [`LANGUAGE`].

use egui::epaint::ClippedShape;
use egui::{Event, PointerButton, Pos2, RawInput, Rect, Shape, Vec2, pos2};
use crossfx_core::i18n::{self, LANGUAGES};
use crossfx_core::{
    AppKey, AudioDevice, DeviceDirection, InterfaceFont, Platform, Settings, ThemeMode, ViewMode,
    WindowsParity,
};
use crossfx_ui::dialogs::settings::{
    self, AppLane, AppRow, DevicePriority, FontMessage, FontPane, GilroyFound, NavIcons,
    SettingsDialog, SettingsState, SettingsTab, WINDOW_SIZE,
};
use crossfx_ui::state::PresetEntry;
use crossfx_ui::{AssetCache, Palette, UiState, ViewScratch, layout, typeface, views};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// The process's language, one test at a time.
static LANGUAGE: Mutex<()> = Mutex::new(());

fn hold_the_language() -> MutexGuard<'static, ()> {
    LANGUAGE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One text a frame painted.
#[derive(Debug, Clone)]
struct Painted {
    text: String,
    rect: Rect,
    clip: Rect,
    elided: bool,
    rows: usize,
}

/// What a window drawn on a `size` screen painted, after it settled: `frames` are the events of
/// each frame — empty ones settle it — and `draw` is the window.
fn paint(size: Vec2, frames: &[Vec<Event>], mut draw: impl FnMut(&mut egui::Ui)) -> Vec<Painted> {
    let ctx = egui::Context::default();
    ctx.set_fonts(typeface::definitions(None));
    // Popups fade in over `animation_time`; at zero a painted list is all there.
    ctx.all_styles_mut(|style| style.animation_time = 0.0);
    let mut shapes: Vec<ClippedShape> = Vec::new();
    let settle = [Vec::new(), Vec::new()];
    for events in frames.iter().chain(&settle) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            events: events.clone(),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| draw(ui));
        shapes = std::mem::take(&mut output.shapes);
        output.drop_without_applying_deltas();
    }
    shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            Shape::Text(text) if !text.galley.is_empty() => Some(Painted {
                text: text.galley.text().to_owned(),
                // The lines' own box, not their ink: a Thai tone mark or an Arabic swash may
                // reach a point past the line it is set on, as it does in any text.
                rect: text.galley.rect.translate(text.pos.to_vec2()),
                clip: clipped.clip_rect,
                elided: text.galley.elided,
                rows: text.galley.rows.len(),
            }),
            _ => None,
        })
        .collect()
}

/// The frames of a click at `pos`: hover, press, release — egui hit-tests against the previous
/// frame's rectangles.
fn click(pos: Pos2) -> Vec<Vec<Event>> {
    let press = |pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    vec![
        vec![Event::PointerMoved(pos)],
        vec![Event::PointerMoved(pos), press(true)],
        vec![press(false)],
        // Away again, so that no hover state is painted over what is measured.
        vec![Event::PointerGone],
    ]
}

/// Room a line's box may be overrun by before it counts: egui sets text on the half-pixel grid.
const SLACK: f32 = 0.5;

/// What is wrong with what `window` painted on a `size` screen, a line each. `allowed` says
/// which cuts the design makes on purpose ([`designed_cut`]).
fn problems(
    window: &str,
    size: Vec2,
    painted: &[Painted],
    allowed: impl Fn(&Painted) -> bool,
) -> Vec<String> {
    let screen = Rect::from_min_size(Pos2::ZERO, size).expand(SLACK);
    let mut found = Vec::new();
    // Only what can be seen: a row scrolled out of its list is not on screen at all.
    let shown: Vec<&Painted> = painted
        .iter()
        .filter(|p| p.rect.intersects(p.clip) && p.clip.is_positive())
        .collect();
    for p in &shown {
        if p.elided && !allowed(p) {
            found.push(format!(
                "{window}: {:?} is cut to fit {:.1} points",
                p.text,
                p.rect.width()
            ));
        }
        // Sideways, any text: nothing scrolls sideways. Downwards, a wrapped one: a single line
        // half under a list's edge is a row scrolled out of sight, but a note's last lines cut
        // off by its own box are lines nobody can read.
        let sideways =
            p.rect.left() < p.clip.left() - SLACK || p.rect.right() > p.clip.right() + SLACK;
        let downwards = p.rows > 1 && p.rect.bottom() > p.clip.bottom() + SLACK;
        if (sideways || downwards) && !allowed(p) {
            found.push(format!(
                "{window}: {:?} runs past its box: {:?} in {:?}",
                p.text, p.rect, p.clip
            ));
        }
        if !screen.contains_rect(p.rect.intersect(p.clip)) {
            found.push(format!(
                "{window}: {:?} is outside the window: {:?}",
                p.text, p.rect
            ));
        }
    }
    // Two texts on top of each other in the same region — a list that opens over the window is a
    // region of its own, and covers what it opens over — by more than a sliver: the lines' boxes
    // include the leading above and below the letters, which may touch.
    for (i, a) in shown.iter().enumerate() {
        for b in shown[i + 1..].iter().filter(|b| b.clip == a.clip) {
            let overlap = a.rect.intersect(a.clip).intersect(b.rect.intersect(b.clip));
            if overlap.is_positive() && overlap.width() > 1.0 && overlap.height() > 3.0 {
                found.push(format!(
                    "{window}: {:?} and {:?} overlap by {:.1} × {:.1}",
                    a.text,
                    b.text,
                    overlap.width(),
                    overlap.height()
                ));
            }
        }
    }
    found
}

/// The cuts the design makes on purpose.
///
/// - The Applications pane's "CrossFX's preset" in the narrow combo of a row: the languages
///   `FOLLOW_PRESET_ELIDED` lists are cut there, each held exactly by the pane's own test
///   (`dialogs::settings::tests::FOLLOW_PRESET_ELIDED`).
/// - "Select preset", the placeholder of a device row's 150-point preset combo
///   (`FxOutputPreference.cpp`): the original's combos cut a label too long for them
///   (`FxTheme.cpp:130`), and twenty-one languages are — the same twenty-one in Gilroy as in
///   Manrope, each a little shorter in Manrope.
/// - From «Like the original FxSound LLC» = Interface on, the Settings tabs the original has are
///   captioned on one line in the original's box and cut there, as in the original (#29).
/// - Polish "Duplicate presets not imported" over the import summary's second list: the
///   original's own table makes it too long even for JUCE's squeeze, and it is cut at seven
///   tenths as it is there (the summary's own test says so too).
fn designed_cut(window: &str, painted: &Painted) -> bool {
    use crossfx_ui::dialogs::presets::SKIPPED_LABEL;
    let follow = i18n::tr(settings::FOLLOW_PRESET);
    (window.contains("applications") && painted.text == follow)
        || (window.contains("settings") && painted.text == i18n::tr("Select preset"))
        || (window.starts_with("pl import") && painted.text == i18n::tr(SKIPPED_LABEL))
        || (window.contains("interface")
            && ["Audio", "General", "Help"]
                .into_iter()
                .any(|tab| painted.text == i18n::tr(tab)))
}

// ---------------------------------------------------------------------------------------------
// The windows, over a made-up state whose own names fit: a cut here is a label's
// ---------------------------------------------------------------------------------------------

fn device(id: u32, description: &str, direction: DeviceDirection) -> AudioDevice {
    AudioDevice {
        id,
        name: format!("fits.{id}"),
        description: description.to_owned(),
        is_default: id == 1,
        direction,
        form_factor: String::new(),
        is_virtual: false,
    }
}

fn main_state(view: ViewMode, direction: DeviceDirection, parity: WindowsParity) -> UiState {
    let presets = ["General", "Music", "Movies", "Gaming", "Mix"]
        .into_iter()
        .enumerate()
        .map(|(index, name)| PresetEntry {
            name: name.to_owned(),
            factory: index < 4,
            modified: false,
        })
        .collect();
    UiState {
        view,
        direction,
        windows_parity: parity,
        presets,
        selected_preset: Some(1),
        devices: vec![
            device(1, "Speakers", DeviceDirection::Output),
            device(2, "Headset", DeviceDirection::Output),
            device(3, "USB Mic", DeviceDirection::Input),
        ],
        selected_output: Some(0),
        selected_input: Some(2),
        effects: [3.0, 5.0, 7.0, 4.0, 8.0],
        master_gain_db: -4.0,
        volume_leveling: 1.5,
        balance_db: 6.0,
        audio_active: true,
        output_active: true,
        input_active: true,
        gate_on: true,
        compressor_on: true,
        deesser_on: true,
        denoise_on: true,
        denoise_running: true,
        deesser_running: true,
        ..UiState::default()
    }
}

fn priority(id: &str, name: &str, preset: Option<usize>) -> DevicePriority {
    DevicePriority {
        id: id.to_owned(),
        name: name.to_owned(),
        preset,
        connected: true,
        present: true,
    }
}

fn settings_state(platform: Platform, tab: SettingsTab, parity: WindowsParity) -> SettingsState {
    let app = |name: &str, lanes: &[(DeviceDirection, Option<&str>)]| AppRow {
        app: AppKey {
            binary: name.to_lowercase(),
            name: name.to_owned(),
            flatpak: String::new(),
        },
        name: name.to_owned(),
        running: true,
        lanes: lanes
            .iter()
            .map(|&(direction, preset)| AppLane {
                direction,
                preset: preset.map(str::to_owned),
            })
            .collect(),
    };
    let mut state = SettingsState {
        tab,
        platform,
        version: "1.0.0".to_owned(),
        presets: ["General", "Music", "Movies", "Gaming"]
            .map(str::to_owned)
            .to_vec(),
        input_presets: ["Voice", "Headset"].map(str::to_owned).to_vec(),
        apps: vec![
            app("Firefox", &[(DeviceDirection::Output, None)]),
            app(
                "Discord",
                &[
                    (DeviceDirection::Output, None),
                    (DeviceDirection::Input, Some("Headset")),
                ],
            ),
        ],
        devices: vec![
            priority("out.a", "Speakers", Some(1)),
            priority("out.b", "Headset", None),
        ],
        microphones: vec![priority("in.a", "USB Mic", Some(0))],
        has_microphone: true,
        ..SettingsState::new(Settings::default())
    };
    state.settings.windows_parity = parity;
    state
}

/// Every text of the main window in `view`, editing `direction`, with `open` clicked first.
fn main_window(state: &UiState, open: Option<Pos2>) -> Vec<Painted> {
    let palette = Palette::new(ThemeMode::Dark).windows(state.windows_parity.interface());
    let mut scratch = ViewScratch::new();
    let mut assets = AssetCache::new();
    let frames = open.map(click).unwrap_or_default();
    paint(views::window_size(state.view), &frames, |ui| {
        let _ = views::show(ui, state, &mut scratch, palette, &mut assets);
    })
}

/// Every text of the Settings window over `state`.
fn settings_window(state: &SettingsState) -> Vec<Painted> {
    let palette = Palette::new(ThemeMode::Dark)
        .windows(state.settings.windows_parity.offered_or_below().interface());
    let mut icons = NavIcons::new();
    let mut assets = AssetCache::new();
    paint(WINDOW_SIZE, &[], |ui| {
        let outer = Rect::from_min_size(Pos2::ZERO, WINDOW_SIZE);
        let _ = SettingsDialog::new(state).show(ui, outer, palette, &mut assets, &mut icons);
    })
}

fn parity_name(parity: WindowsParity) -> &'static str {
    match parity {
        WindowsParity::Off => "off",
        WindowsParity::Interface => "interface",
        WindowsParity::Sound => "sound",
        WindowsParity::Full => "full",
    }
}

fn tab_name(tab: SettingsTab) -> &'static str {
    match tab {
        SettingsTab::Audio => "audio",
        SettingsTab::General => "general",
        SettingsTab::Help => "help",
        SettingsTab::Microphone => "microphone",
        SettingsTab::Applications => "applications",
        SettingsTab::Experimental => "experimental",
    }
}

/// Run `audit` in every language, and fail with everything any of them found.
fn in_every_language(audit: impl Fn(&str) -> Vec<String>) {
    let _language = hold_the_language();
    let mut found = Vec::new();
    for language in &LANGUAGES {
        assert!(i18n::set_language(language.code), "{}", language.code);
        found.extend(audit(language.code));
    }
    i18n::set_language("en");
    assert!(
        found.is_empty(),
        "{} lines do not fit:\n  {}",
        found.len(),
        found.join("\n  ")
    );
}

// ---------------------------------------------------------------------------------------------
// The audit
// ---------------------------------------------------------------------------------------------

#[test]
fn every_language_fits_the_main_windows_and_their_open_lists() {
    in_every_language(|code| {
        let mut found = Vec::new();
        for parity in [WindowsParity::Off, WindowsParity::Interface] {
            for view in [ViewMode::Pro, ViewMode::Lite] {
                for direction in [DeviceDirection::Output, DeviceDirection::Input] {
                    let state = main_state(view, direction, parity);
                    let (preset_combo, device_combo) = match view {
                        ViewMode::Pro => (
                            layout::pro::preset_combo(),
                            layout::pro::device_combo(direction),
                        ),
                        ViewMode::Lite => {
                            (layout::lite::preset_combo(), layout::lite::output_combo())
                        }
                    };
                    for (open, what) in [
                        (None, "closed"),
                        (Some(preset_combo.center()), "presets open"),
                        (Some(device_combo.center()), "devices open"),
                    ] {
                        let window = format!(
                            "{code} {view:?} {direction:?} {} {what}",
                            parity_name(parity)
                        );
                        let size = views::window_size(view);
                        let painted = main_window(&state, open);
                        found.extend(problems(&window, size, &painted, |p| {
                            designed_cut(&window, p)
                        }));
                    }
                }
            }
        }
        found
    });
}

#[test]
fn every_language_fits_every_settings_pane_on_both_platforms() {
    in_every_language(|code| {
        let mut found = Vec::new();
        for platform in [Platform::LINUX, Platform::WINDOWS] {
            let system = if platform == Platform::LINUX {
                "linux"
            } else {
                "windows"
            };
            for parity in [
                WindowsParity::Off,
                WindowsParity::Interface,
                WindowsParity::Sound,
            ] {
                for tab in SettingsTab::ALL {
                    if !tab.offered_on(platform) {
                        continue;
                    }
                    let state = settings_state(platform, tab, parity);
                    let window = format!(
                        "{code} {system} settings-{} {}",
                        tab_name(tab),
                        parity_name(parity)
                    );
                    found.extend(problems(
                        &window,
                        WINDOW_SIZE,
                        &settings_window(&state),
                        |p| designed_cut(&window, p),
                    ));
                }
            }
        }
        found
    });
}

/// One thing the General pane can have to say under "Interface font".
struct FontCase {
    what: &'static str,
    font: InterfaceFont,
    pane: FontPane,
    parity: WindowsParity,
}

#[test]
fn every_language_fits_what_the_general_pane_says_about_gilroy() {
    // Gilroy chosen and none found: the explanation, the two buttons and the note, the pane
    // scrolling under them. And the other things the lines under the option can say.
    let pane = |gilroy, lacks_letters, message| FontPane {
        gilroy,
        lacks_letters,
        message,
    };
    let cases = [
        FontCase {
            what: "missing",
            font: InterfaceFont::Gilroy,
            pane: pane(GilroyFound::None, false, None),
            parity: WindowsParity::Off,
        },
        FontCase {
            what: "incomplete, a file left out",
            font: InterfaceFont::Gilroy,
            pane: pane(
                GilroyFound::Incomplete,
                false,
                Some(FontMessage::LeftOut(vec!["Gilroy-Light.ttf".to_owned()])),
            ),
            parity: WindowsParity::Off,
        },
        FontCase {
            what: "found",
            font: InterfaceFont::Gilroy,
            pane: pane(GilroyFound::Whole, false, None),
            parity: WindowsParity::Off,
        },
        FontCase {
            what: "found without the letters",
            font: InterfaceFont::Gilroy,
            pane: pane(GilroyFound::Whole, true, Some(FontMessage::NotCopied)),
            parity: WindowsParity::Off,
        },
        FontCase {
            what: "manrope, original look",
            font: InterfaceFont::Manrope,
            pane: pane(GilroyFound::Whole, false, Some(FontMessage::NoBrowser)),
            parity: WindowsParity::Interface,
        },
    ];
    in_every_language(|code| {
        let mut found = Vec::new();
        for platform in [Platform::LINUX, Platform::WINDOWS] {
            let system = if platform == Platform::LINUX {
                "linux"
            } else {
                "windows"
            };
            for case in &cases {
                let mut state = settings_state(platform, SettingsTab::General, case.parity);
                state.settings.interface_font = case.font;
                state.font = case.pane.clone();
                let window = format!(
                    "{code} {system} settings-general {} {}",
                    case.what,
                    parity_name(case.parity)
                );
                found.extend(problems(
                    &window,
                    WINDOW_SIZE,
                    &settings_window(&state),
                    |p| designed_cut(&window, p),
                ));
            }
        }
        found
    });
}

#[test]
fn every_language_fits_the_preset_windows_the_calibration_and_the_message_boxes() {
    use crossfx_ui::dialogs::calibration::{
        CalibrationDialog, CalibrationPhase, CalibrationResultView, CalibrationView,
    };
    use crossfx_ui::dialogs::message::{self, ConfirmStyle, MessageBox};
    use crossfx_ui::dialogs::presets::{
        ExportDialog, ExportState, ImportDialog, ImportState, ImportSummary, export,
    };
    let palette = Palette::new(ThemeMode::Dark);
    // One window drawn into `size`, with what it painted audited as `window`.
    let audit =
        |window: String, size: Vec2, draw: &mut dyn FnMut(&mut egui::Ui, Rect, &mut AssetCache)| {
            let mut assets = AssetCache::new();
            let outer = Rect::from_min_size(Pos2::ZERO, size);
            let painted = paint(size, &[], |ui| draw(ui, outer, &mut assets));
            problems(&window, size, &painted, |p| designed_cut(&window, p))
        };
    in_every_language(|code| {
        let mut found = Vec::new();
        for import in [
            ImportState::default(),
            ImportState {
                folder: Some("/home/me/Presets".into()),
                ..ImportState::default()
            },
            ImportState {
                summary: Some(ImportSummary {
                    imported: vec!["Club".to_owned(), "Lounge".to_owned()],
                    skipped: vec!["Music".to_owned()],
                }),
                ..ImportState::default()
            },
        ] {
            let dialog = ImportDialog::new(&import);
            let size = dialog.window_size();
            found.extend(audit(
                format!("{code} import"),
                size,
                &mut |ui, outer, assets| {
                    let _ = ImportDialog::new(&import).show(ui, outer, palette, assets, "fits");
                },
            ));
        }
        for offered in [false, true] {
            let state = ExportState {
                presets: ["Club", "Lounge", "Music"].map(str::to_owned).to_vec(),
                selected: [0].into_iter().collect(),
                end_bands_offered: offered,
                ..ExportState::default()
            };
            found.extend(audit(
                format!("{code} export"),
                export::WINDOW_SIZE,
                &mut |ui, outer, assets| {
                    let _ = ExportDialog::new(&state).show(ui, outer, palette, assets, "fits");
                },
            ));
        }
        for phase in CalibrationPhase::ALL {
            let view = CalibrationView {
                phase,
                seconds_left: 3.0,
                phase_fraction: 0.4,
                level_db: -30.0,
                device: "USB Mic".to_owned(),
                failure: i18n::tr("No speech was heard. Speak closer to the microphone."),
                can_measure: true,
                result: (phase == CalibrationPhase::Result).then(|| CalibrationResultView {
                    floor_db: -62.0,
                    speech_rms_db: -24.0,
                    speech_peak_db: -6.0,
                    clipped_percent: 0.0,
                    preset: "Voice".to_owned(),
                    lines: vec![
                        i18n::tr_args("High-pass %s", &["80 Hz"]),
                        i18n::tr_args("Gate %s", &["−54 dB"]),
                        i18n::tr_args("Compressor %s, ratio %s", &["−20 dB", "3:1"]),
                        i18n::tr_args("Makeup gain %s", &["4 dB"]),
                        i18n::tr_args("Ceiling %s", &["−1 dB"]),
                    ],
                }),
            };
            found.extend(audit(
                format!("{code} calibration {phase:?}"),
                view.window_size(),
                &mut |ui, outer, assets| {
                    let _ = CalibrationDialog::new(&view).show(ui, outer, palette, assets, "fits");
                },
            ));
        }
        let question = i18n::tr_args("Move the preset %s to the trash?", &["Lounge"]);
        let notice = i18n::tr("Preset files not found in the selected folder.");
        for (text, style) in [(question, ConfirmStyle::YesNo), (notice, ConfirmStyle::Ok)] {
            found.extend(audit(
                format!("{code} message"),
                message::WINDOW_SIZE,
                &mut |ui, outer, assets| {
                    let _ = MessageBox::new(&text)
                        .style(style)
                        .show(ui, outer, palette, assets, "fits");
                },
            ));
        }
        found
    });
}

#[test]
fn the_audit_sees_a_cut_a_clipped_note_an_overlap_and_text_off_the_window() {
    // An audit that had gone blind would pass, which is the one way it must not fail.
    let size = Vec2::new(100.0, 100.0);
    let line = |text: &str, rect: Rect, clip: Rect, elided: bool, rows: usize| Painted {
        text: text.to_owned(),
        rect,
        clip,
        elided,
        rows,
    };
    let screen = Rect::from_min_size(Pos2::ZERO, size);
    let box_ = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(pos2(x, y), Vec2::new(w, h));
    let none = |_: &Painted| false;
    // Each alone, then each found.
    assert!(
        problems(
            "w",
            size,
            &[line("ok", box_(0.0, 0.0, 20.0, 10.0), screen, false, 1)],
            none
        )
        .is_empty()
    );
    let cut = problems(
        "w",
        size,
        &[line(
            "Lautstärke",
            box_(0.0, 0.0, 20.0, 10.0),
            screen,
            true,
            1,
        )],
        none,
    );
    assert!(cut.len() == 1 && cut[0].contains("cut"), "{cut:?}");
    let clipped = problems(
        "w",
        size,
        &[line(
            "two lines",
            box_(0.0, 0.0, 20.0, 30.0),
            box_(0.0, 0.0, 20.0, 15.0),
            false,
            2,
        )],
        none,
    );
    assert!(
        clipped.len() == 1 && clipped[0].contains("runs past"),
        "{clipped:?}"
    );
    let over = problems(
        "w",
        size,
        &[
            line("left", box_(0.0, 0.0, 30.0, 10.0), screen, false, 1),
            line("right", box_(20.0, 0.0, 30.0, 10.0), screen, false, 1),
        ],
        none,
    );
    assert!(over.len() == 1 && over[0].contains("overlap"), "{over:?}");
    let outside = problems(
        "w",
        size,
        &[line(
            "far",
            box_(90.0, 0.0, 30.0, 10.0),
            Rect::EVERYTHING,
            false,
            1,
        )],
        none,
    );
    assert!(
        outside.len() == 1 && outside[0].contains("outside"),
        "{outside:?}"
    );
    // And a designed cut is let through.
    assert!(
        problems(
            "w",
            size,
            &[line("x", box_(0.0, 0.0, 20.0, 10.0), screen, true, 1)],
            |_| true
        )
        .is_empty()
    );
}
