//! Persisted application settings.
//!
//! The original FxSound for Windows keeps these in a JUCE `PropertiesFile` XML at
//! `%APPDATA%\FxSound\FxSound.settings`, its own folder, which CrossFX never reads. CrossFX keeps
//! the **same key names** in a TOML file at `$XDG_CONFIG_HOME/crossfx/settings.toml` (default
//! `~/.config/crossfx/settings.toml`; on Windows `%APPDATA%\CrossFX\settings.toml`) so the schema
//! stays greppable against the original C++ and a user's settings are recognisable.
//!
//! The original's keys nothing here reads are not settings of CrossFX's (0.4.0 audit #35): the
//! five hotkey chords and the `hotkeys` switch (a Wayland client cannot grab global shortcuts; the
//! compositor binds them to the command line), `window_x`/`window_y` (Wayland gives a client no way
//! to place its own toplevel), `always_on_top` (winit ignores window levels there),
//! `automatic_updates` and `last_update_time` (CrossFX never contacts the network), and
//! `device_configs_version`. CrossFX writes none of them; a file that has one keeps it, as it keeps
//! every key it does not know ([`Settings::extra`]). `run_minimized` is written whenever the window
//! hides to the tray or shows.
//!
//! A file that does not load — it does not parse, it is not UTF-8, this user may not read it — is
//! moved aside as `settings.toml.bad` (`settings.toml.2.bad` and so on when that is taken) and the
//! defaults are used — never overwritten in place, so a hand edit that went wrong can still be
//! read back by the person who made it.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::messages::TargetVolume;
use crate::parity::WindowsParity;
use crate::{
    DeEsserMode, DenoiseChannelsOverride, DereverbLevel, DeviceDirection, NoiseSuppressionOverride,
};

/// How many user presets `max_user_presets` may allow (audit report #20). The floor is the
/// original's; the ceiling was 120 there, raised because a preset is a few hundred bytes and the
/// limit only exists to keep the tray's submenu finite.
pub const USER_PRESET_LIMITS: std::ops::RangeInclusive<u32> = 10..=1000;

/// Which of the two window layouts is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ViewMode {
    /// The full window: visualizer, effect sliders and the graphic equalizer.
    #[default]
    Pro,
    /// The compact window: preset and output pickers only.
    Lite,
}

/// Light or dark palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
}

/// The face the interface is set in: Settings ▸ General ▸ "Interface font".
///
/// Manrope is CrossFX's own, embedded in the binary under the SIL Open Font License. Gilroy is the
/// face of the original FxSound: Radomir Tinkov's commercial typeface, which CrossFX neither ships
/// nor downloads. It is used only when the files the user licensed are found — added from the
/// pane, or installed on the system (`crossfx_ui::typeface`) — and only then: chosen and not
/// found, the interface stays in Manrope.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum InterfaceFont {
    /// Manrope, the default.
    #[default]
    Manrope,
    /// Gilroy, the original FxSound look, when the user has it.
    Gilroy,
}

impl InterfaceFont {
    /// Both faces, in the order the option steps through them.
    pub const ALL: [Self; 2] = [Self::Manrope, Self::Gilroy];

    /// The spelling in `settings.toml`.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Manrope => "manrope",
            Self::Gilroy => "gilroy",
        }
    }

    /// The face's own name, as the option shows it. A name, so never translated.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Manrope => "Manrope",
            Self::Gilroy => "Gilroy",
        }
    }

    /// A face by its key, in any case and with the blanks around it ignored.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        Self::ALL
            .into_iter()
            .find(|font| text.eq_ignore_ascii_case(font.key()))
    }

    /// The face a `settings.toml` value stands for. Never an error, for the reason
    /// [`WindowsParity::from_toml`] is not one: a value this version cannot read is Manrope, with a
    /// warning in the log, and costs this one key rather than the file.
    #[must_use]
    pub fn from_toml(value: &toml::Value) -> Self {
        match value {
            toml::Value::String(text) => Self::parse(text).unwrap_or_else(|| {
                log::warn!(
                    "settings.toml: interface_font = {text:?} is not manrope or gilroy; using \
                     manrope"
                );
                Self::Manrope
            }),
            other => {
                log::warn!(
                    "settings.toml: interface_font = {other} is not manrope or gilroy; using manrope"
                );
                Self::Manrope
            }
        }
    }
}

impl Serialize for InterfaceFont {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.key())
    }
}

/// Never fails: see [`InterfaceFont::from_toml`].
impl<'de> Deserialize<'de> for InterfaceFont {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = toml::Value::deserialize(deserializer)?;
        Ok(Self::from_toml(&value))
    }
}

#[cfg(test)]
mod interface_font_tests {
    use super::*;

    #[test]
    fn a_file_written_before_the_key_existed_reads_as_manrope() {
        let settings: Settings =
            toml::from_str("theme_mode = \"light\"\nlanguage = \"ru\"\n").expect("parses");
        assert_eq!(settings.interface_font, InterfaceFont::Manrope);
        assert_eq!(settings.theme_mode, ThemeMode::Light);
        assert_eq!(Settings::default().interface_font, InterfaceFont::Manrope);
    }

    #[test]
    fn the_key_reads_in_any_case_and_a_value_it_cannot_read_costs_only_itself() {
        let read = |text: &str| -> Settings { toml::from_str(text).expect("never fails") };
        assert_eq!(
            read("interface_font = \"gilroy\"").interface_font,
            InterfaceFont::Gilroy
        );
        assert_eq!(
            read("interface_font = \" Gilroy \"").interface_font,
            InterfaceFont::Gilroy
        );
        for value in [
            "\"inter\"",
            "1",
            "true",
            "[\"gilroy\"]",
            "{ face = \"gilroy\" }",
        ] {
            let settings = read(&format!("language = \"de\"\ninterface_font = {value}\n"));
            assert_eq!(settings.interface_font, InterfaceFont::Manrope, "{value}");
            assert_eq!(settings.language, "de", "{value} cost the rest of the file");
        }
    }

    #[test]
    fn the_choice_is_written_under_its_key_and_read_back() {
        let settings = Settings {
            interface_font: InterfaceFont::Gilroy,
            ..Settings::default()
        };
        let text = toml::to_string_pretty(&settings).expect("serialises");
        assert!(text.contains("interface_font = \"gilroy\""), "{text}");
        let back: Settings = toml::from_str(&text).expect("parses");
        assert_eq!(back.interface_font, InterfaceFont::Gilroy);
        // Every face's key is its own, and its name is the face's, capitalised.
        for font in InterfaceFont::ALL {
            assert_eq!(InterfaceFont::parse(font.key()), Some(font));
            assert!(font.name().eq_ignore_ascii_case(font.key()));
        }
    }

    #[test]
    fn the_added_fonts_are_kept_beside_the_presets_in_crossfx_own_data_folder() {
        let fonts = Settings::user_font_dir();
        assert!(fonts.ends_with(std::path::Path::new(APP_DIR_NAME).join("fonts")));
        assert_eq!(fonts.parent(), Settings::user_preset_dir().parent());
    }
}

#[cfg(test)]
mod folder_tests {
    use std::path::{Path, PathBuf};

    use super::*;

    /// Whether any component of `path` is `fxsound` in any case: on Windows, where case is not
    /// told apart, the original FxSound's own folder.
    fn names_the_original(path: &Path) -> bool {
        path.components()
            .any(|part| part.as_os_str().eq_ignore_ascii_case("fxsound"))
    }

    /// Every folder and file CrossFX builds for itself here, as the functions build them. Only
    /// built: nothing is created, read or listed.
    fn own_paths() -> Vec<PathBuf> {
        let mut paths = vec![
            Settings::config_dir(),
            Settings::config_path(),
            Settings::user_preset_dir(),
            Settings::user_font_dir(),
            crate::AppRules::config_path(),
        ];
        paths.extend(Settings::local_dir());
        paths
    }

    #[test]
    fn every_folder_of_crossfx_is_its_own_under_the_system_directory() {
        let config = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        let data = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
        assert_eq!(Settings::config_dir(), config.join(APP_DIR_NAME));
        assert_eq!(
            Settings::config_path(),
            config.join(APP_DIR_NAME).join("settings.toml")
        );
        assert_eq!(
            Settings::user_preset_dir(),
            data.join(APP_DIR_NAME).join("presets")
        );
        assert_eq!(
            Settings::user_font_dir(),
            data.join(APP_DIR_NAME).join("fonts")
        );
        assert_eq!(
            Settings::local_dir(),
            dirs::data_local_dir().map(|local| local.join(APP_DIR_NAME))
        );
        assert_eq!(APP_DIR_NAME, crate::identity::DIR_NAME);
        // What CrossFX adds under the system's directory starts with its own folder and names the
        // original nowhere. (The directory itself is the user's: a home called `fxsound` is theirs.)
        let local = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
        for path in own_paths() {
            let added = [&config, &data, &local]
                .into_iter()
                .find_map(|home| path.strip_prefix(home).ok())
                .unwrap_or_else(|| panic!("{} is under no system directory", path.display()));
            assert!(added.starts_with(APP_DIR_NAME), "{}", path.display());
            assert!(!names_the_original(added), "{}", path.display());
        }
    }

    #[test]
    fn the_check_for_the_originals_folder_ignores_case() {
        assert!(names_the_original(Path::new(
            "C:/Users/u/AppData/Roaming/FxSound/FxSound.settings"
        )));
        assert!(names_the_original(Path::new("/x/FXSOUND")));
        assert!(names_the_original(Path::new("FxSound/Presets")));
        assert!(!names_the_original(Path::new(
            "C:/Users/u/AppData/Roaming/CrossFX/settings.toml"
        )));
    }

    /// On Windows, where `%APPDATA%\FxSound` and `%LOCALAPPDATA%\FxSound` are the original's: every
    /// folder CrossFX builds is `CrossFX` under the user's real roaming or local application data,
    /// and none is the original's in any case. The real homes, but only built, never touched.
    #[cfg(windows)]
    #[test]
    fn on_windows_every_folder_is_crossfx_under_appdata_and_none_is_the_originals() {
        let roaming = dirs::config_dir().expect("the roaming application data");
        let local = dirs::data_local_dir().expect("the local application data");
        assert_eq!(dirs::data_dir().as_deref(), Some(roaming.as_path()));
        for dir in [
            Settings::config_dir(),
            Settings::user_preset_dir()
                .parent()
                .map(Path::to_owned)
                .expect("a parent"),
            Settings::user_font_dir()
                .parent()
                .map(Path::to_owned)
                .expect("a parent"),
            crate::AppRules::config_path()
                .parent()
                .map(Path::to_owned)
                .expect("a parent"),
        ] {
            assert_eq!(dir, roaming.join("CrossFX"), "{}", dir.display());
        }
        assert_eq!(Settings::local_dir(), Some(local.join("CrossFX")));
        for path in own_paths() {
            assert!(!names_the_original(&path), "{}", path.display());
            assert!(
                path.starts_with(&roaming) || path.starts_with(&local),
                "{}",
                path.display()
            );
        }
    }
}

/// One remembered device and the preset the user last used with it.
///
/// Every field defaults, for the same reason the file as a whole does: an entry written by an
/// older version, or one a hand edit left short, must cost that entry's missing field and not
/// the whole settings file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DeviceConfig {
    /// On Windows an endpoint id; here the PipeWire `node.name`, which is stable across restarts.
    pub device_id: String,
    /// Which lane this memory belongs to; an entry without the key, which only a hand edit
    /// leaves, is an output entry. The two directions are separate namespaces, because a `.fac`
    /// name and a voice preset's name can collide.
    #[serde(default)]
    pub direction: DeviceDirection,
    /// Human-readable name (`node.description`).
    pub device_name: String,
    /// Preset last selected while this device was active.
    pub preset: String,
    /// `speaker`, `headphone`, `hdmi`, … as PipeWire spells them.
    ///
    /// Recorded so a device can be recognised by kind rather than only by name. Nothing chooses a
    /// preset from it yet — that would change which preset a user hears when they plug something
    /// in, which is a decision worth making deliberately rather than inheriting from a comment.
    pub device_form_factor: String,
    /// Every key of the entry this version does not know, kept and written back with the entry,
    /// as [`Settings::extra`] keeps the file's: a per-device key a later version adds survives a
    /// save here. An entry this version creates starts with none; one it forgets goes with its
    /// keys.
    #[serde(flatten)]
    pub extra: toml::Table,
}

/// What the microphone calibration wizard last measured and what it did about it.
///
/// Informational: the Settings pane shows it, and the preset the wizard wrote is a file of its
/// own. Nothing here reaches a filter design, so a corrupt record is dropped rather than clamped
/// — a floor of NaN is not a measurement that was merely too large.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CalibrationRecord {
    /// The floor measured during the silence phase, dBFS.
    pub noise_floor_db: f32,
    /// Mean RMS of the speech phase, dBFS.
    pub speech_rms_db: f32,
    /// Peak of the speech phase, dBFS.
    pub speech_peak_db: f32,
    /// Fraction of the loud phase's samples that clipped, `0.0..=1.0`.
    pub clipped_ratio: f32,
    /// When the wizard ran, as seconds since the Unix epoch.
    pub unix_time: u64,
    /// The voice preset the wizard wrote and selected.
    pub preset: String,
    /// `node.name` of the microphone it was run on.
    pub device: String,
    /// Every key of the record this version does not know, kept and written back with it, as
    /// [`Settings::extra`] keeps the file's. They describe the same run as the rest of the record,
    /// so they go with it: when the wizard runs again, and when the record is dropped as corrupt.
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl CalibrationRecord {
    /// Whether every measurement in the record is a number.
    #[must_use]
    fn is_finite(&self) -> bool {
        [
            self.noise_floor_db,
            self.speech_rms_db,
            self.speech_peak_db,
            self.clipped_ratio,
        ]
        .iter()
        .all(|v| v.is_finite())
    }
}

/// The whole settings file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    // ---- audio -----------------------------------------------------------------------------
    /// Master power state, restored on launch.
    pub power: bool,
    /// Name of the preset to select on launch while CrossFX sits in front of a playback device.
    pub output_preset: String,
    /// Name of the preset to select while CrossFX sits behind a microphone.
    ///
    /// Separate because the two chains are separate. A music preset on a voice is wrong by
    /// construction — reverberation, stereo widening and a bass lift are the opposite of what a
    /// voice wants — so carrying one across a direction switch would hand the user a sound nobody
    /// chose. The default is the voice set's own reference preset, not the output set's: a name
    /// that is in neither list means the picker lands on whatever happens to sort first.
    pub input_preset: String,
    /// `node.name` of the real output device CrossFX renders to.
    pub output_device_name: String,
    /// `node.name` of the real capture device CrossFX listens to.
    pub input_device_name: String,
    /// The **edit direction**: which lane the window's preset picker, equalizer, level controls
    /// and meters address. Linux only, and a GUI notion only — both lanes run regardless, and
    /// the engine never sees this. Whether a lane runs is [`Settings::input_enabled`]'s and
    /// [`Settings::output_enabled`]'s, never this key's.
    pub device_direction: DeviceDirection,
    /// Whether the input lane comes up at start. `false` until the user gives the microphone lane
    /// a device: what a fresh install does.
    pub input_enabled: bool,
    /// Whether the output lane comes up at start. `true` unless the user detached it: the
    /// original's behaviour, and what a fresh install does.
    pub output_enabled: bool,
    /// Remembered devices and their presets, in both directions.
    pub device_configs: Vec<DeviceConfig>,
    /// Switch to a newly appeared output device automatically.
    pub prioritize_new_output: bool,
    /// Ignore the device priority list and let the session default decide (U4, the original's
    /// #629).
    ///
    /// Off by default, which is the original's behaviour: the ranked list of `device_configs` picks
    /// the device. On, the app sends an empty ranking, and CrossFX follows whatever the desktop's
    /// own sound settings choose.
    pub follow_system_default: bool,
    /// The volume of CrossFX's own node per real target, per direction (U10).
    ///
    /// One entry per `(direction, target, port)` ([`TargetVolume::same_place`]): the speakers and
    /// the headphones of one sink keep a level each. Without it WirePlumber restores one volume for
    /// `crossfx_sink` whatever it renders to, and a level set for headphones is the level the
    /// speakers get after an unplug. [`Settings::sanitise`] drops an entry that is not a
    /// volume anyone could have set; see [`TargetVolume::sanitised`].
    pub device_volumes: Vec<TargetVolume>,
    /// What the session default was before CrossFX took it, one per direction.
    ///
    /// The audio thread keeps this on its own heap and hands the default back on exit, on a
    /// direction switch and on `SIGTERM`/`SIGINT`. None of that survives a `SIGKILL`, an OOM kill
    /// or a power cut — and what those leave behind is a session default naming CrossFX's virtual
    /// node, which is gone. Written here so the next start can repair it, which is the only place
    /// the repair can live: no signal handler runs for any of the three.
    pub remembered_default_output: String,
    pub remembered_default_input: String,

    // ---- microphone: global overrides ----------------------------------------------------------
    //
    // A voice preset carries its own denoiser level and channel mode; these sit over every
    // preset at once, and `Preset` means "whatever the preset says". Echo cancellation is global
    // because a PipeWire module is per session, not per preset.
    /// Noise-suppression level over every voice preset, or follow the preset.
    pub noise_suppression: NoiseSuppressionOverride,
    /// Denoiser channel mode over every voice preset, or follow the preset.
    pub denoise_channels: DenoiseChannelsOverride,
    /// Whether the de-esser's corner is the preset's or chosen from the source's bandwidth.
    pub deesser_mode: DeEsserMode,
    /// Run PipeWire's echo canceller in front of the input lane.
    pub echo_cancel: bool,
    /// Late-reverberation suppression on the input lane.
    pub dereverb: DereverbLevel,
    /// The calibration wizard's last result, for the Settings pane.
    pub calibration: Option<CalibrationRecord>,

    // ---- dsp state that lives outside the preset --------------------------------------------
    pub master_gain: f32,
    pub balance: f32,
    pub volume_leveling: f32,
    pub filter_q: f32,
    pub num_bands: u32,

    // ---- window ----------------------------------------------------------------------------
    pub view: ViewMode,
    pub theme_mode: ThemeMode,
    /// Start with no window, tray only: whether the window was hidden to the tray when CrossFX
    /// last ran (`FxController.cpp:911-934`, `docs/spec/07-startup-tray.md` §7.1). Written when the
    /// window shows and when it hides into a tray icon that is on screen; `--hide` and `--show`
    /// override it for one start.
    pub run_minimized: bool,

    // ---- ui --------------------------------------------------------------------------------
    /// UI language code (`"en"`, `"de"`, …) — the one the user picked in Settings. Only
    /// consulted while `language_follows_system` is `false`.
    pub language: String,
    /// Show the UI in the desktop session's language rather than in [`Settings::language`].
    /// Linux only: the original has no such notion, so this is what a fresh install does.
    pub language_follows_system: bool,
    pub hide_help_tooltips: bool,
    pub hide_notifications: bool,
    /// How many presets the user may save, from 10 to 1000 ([`USER_PRESET_LIMITS`]).
    pub max_user_presets: u32,
    /// The face the interface is set in, Manrope unless the user chose Gilroy. A file written
    /// before 1.0 has no such key and reads as Manrope; what is drawn is Manrope too while Gilroy
    /// is chosen and not found ([`InterfaceFont`]).
    pub interface_font: InterfaceFont,

    // ---- like the original FxSound LLC ---------------------------------------------------
    /// «Как в оригинальном FxSound LLC» / "Like the original FxSound LLC"
    /// (`docs/0.5.0-windows-parity.md`). Written only
    /// when it is not `off`, and read without ever failing: a value it cannot understand is Off
    /// ([`WindowsParity::from_toml`]), never a reason to move the file aside. Everything (`full`),
    /// which 0.5.0 does not offer, is kept here as it was read and written back by every save, so
    /// going back up to 0.6.0 finds it; what runs is [`WindowsParity::offered_or_below`] of it,
    /// Interface and sound (`App::windows_parity`).
    #[serde(skip_serializing_if = "WindowsParity::is_off")]
    pub windows_parity: WindowsParity,
    /// Export a `.fac` with its end bands where they are, limited only to the equalizer's
    /// 10 Hz–21 kHz, instead of back inside the range the original tunes them in (0.4.0
    /// audit R6): the export window's choice and `--export-unshifted`. Offered, and followed, from
    /// «Like the original FxSound LLC» = Interface and sound on; below it every export shifts
    /// them, as
    /// 0.4.0 does. Written only when set.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub export_unshifted: bool,

    /// Every key of the file this version does not know, kept and written back as it was read.
    /// This is the file's top level; a key inside an entry of `device_configs` or
    /// `device_volumes`, or inside `[calibration]`, is kept by that entry's own `extra`.
    ///
    /// A key written by a later version — a customisation key read by this one after a
    /// downgrade — survives a save here, so going back up a version finds it again.
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            power: true,
            output_preset: "General".into(),
            input_preset: "Clean Voice".into(),
            output_device_name: String::new(),
            input_device_name: String::new(),
            device_direction: DeviceDirection::Output,
            input_enabled: false,
            output_enabled: true,
            device_configs: Vec::new(),
            prioritize_new_output: false,
            follow_system_default: false,
            device_volumes: Vec::new(),
            remembered_default_output: String::new(),
            remembered_default_input: String::new(),

            noise_suppression: NoiseSuppressionOverride::Preset,
            denoise_channels: DenoiseChannelsOverride::Preset,
            deesser_mode: DeEsserMode::Classic,
            echo_cancel: false,
            dereverb: DereverbLevel::Off,
            calibration: None,

            master_gain: 0.0,
            balance: 0.0,
            volume_leveling: 0.0,
            filter_q: 1.0,
            num_bands: crate::eq::DEFAULT_BANDS as u32,

            view: ViewMode::Pro,
            theme_mode: ThemeMode::Dark,
            run_minimized: false,

            language: "en".into(),
            language_follows_system: true,
            hide_help_tooltips: false,
            hide_notifications: false,
            max_user_presets: 120,
            interface_font: InterfaceFont::Manrope,

            windows_parity: WindowsParity::Off,
            export_unshifted: false,
            extra: toml::Table::new(),
        }
    }
}

/// CrossFX's folder under the system's configuration and data directories:
/// [`crate::identity::DIR_NAME`], `crossfx`, and on Windows `CrossFX`.
///
/// `%APPDATA%\FxSound` and `%LOCALAPPDATA%\FxSound` are the original FxSound's, FxSound LLC's,
/// with its settings and its user presets in them, and Windows does not tell `fxsound` from
/// `FxSound`: CrossFX keeps out of the original's folders there, as its installers keep out of
/// its name. Each of CrossFX's folders is made by the code that first writes into it; nothing an
/// earlier version left is read into them.
pub const APP_DIR_NAME: &str = crate::identity::DIR_NAME;

impl Settings {
    /// `~/.config/crossfx` (or `$XDG_CONFIG_HOME/crossfx`); on Windows `%APPDATA%\CrossFX`
    /// ([`APP_DIR_NAME`]).
    #[must_use]
    pub fn config_dir() -> std::path::PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(APP_DIR_NAME)
    }

    /// `~/.config/crossfx/settings.toml`; on Windows `%APPDATA%\CrossFX\settings.toml`.
    #[must_use]
    pub fn config_path() -> std::path::PathBuf {
        Self::config_dir().join("settings.toml")
    }

    /// `~/.local/share/crossfx/presets` — where imported and user-saved presets land; on Windows
    /// `%APPDATA%\CrossFX\presets`, never the original's `%APPDATA%\FxSound\Presets`.
    #[must_use]
    pub fn user_preset_dir() -> std::path::PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(APP_DIR_NAME)
            .join("presets")
    }

    /// `~/.local/share/crossfx/fonts` (`$XDG_DATA_HOME/crossfx/fonts`; on Windows
    /// `%APPDATA%\CrossFX\fonts`, [`APP_DIR_NAME`]) — where the Gilroy files the user added in
    /// Settings are kept, beside the presets, and read from at every start ([`InterfaceFont`]).
    #[must_use]
    pub fn user_font_dir() -> std::path::PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(APP_DIR_NAME)
            .join("fonts")
    }

    /// CrossFX's folder in the data that stays on this machine: `%LOCALAPPDATA%\CrossFX` on
    /// Windows, where the record of the default devices CrossFX holds and the toast's icon are
    /// kept, and `~/.local/share/crossfx` (`$XDG_DATA_HOME/crossfx`) elsewhere, where the system
    /// makes no such distinction. `None` when the system names no such directory.
    ///
    /// Never the original's `%LOCALAPPDATA%\FxSound`. On Windows the folder is the one the shell
    /// names (`dirs::data_local_dir`, the known folder), not the `LOCALAPPDATA` variable: whatever
    /// the environment says, what CrossFX keeps here is in a folder of its own name.
    #[must_use]
    pub fn local_dir() -> Option<std::path::PathBuf> {
        dirs::data_local_dir().map(|dir| dir.join(APP_DIR_NAME))
    }

    /// Load the settings file, falling back to defaults for anything missing or unreadable.
    ///
    /// A corrupt file is never fatal: the user's audio should still come up.
    #[must_use]
    pub fn load() -> Self {
        Self::load_from(&Self::config_path())
    }

    /// [`Settings::load`] from an explicit path — the seam the tests use.
    ///
    /// A file that is there but does not load is **moved aside**, to `<path>.bad`, before the
    /// defaults are returned: one that does not parse, one that is not UTF-8 (a comment saved in
    /// CP1251 or Latin-1 by an editor on a legacy encoding), one this user may not read (after a
    /// `chmod`, or a copy root owns). Silently resetting was the 0.3.0 behaviour, and it had a
    /// cost that grew with every key added: one typo in a hand edit — `power = "yes"` — and the
    /// next save (a slider let go, a preset picked, the way out) renamed the defaults over every
    /// device, preset and language the user had, with only a log line to say so. Now the next
    /// save writes a fresh file beside the broken one, and the broken one is there to be read
    /// back. An earlier `.bad` is never replaced: this one goes to `<path>.2.bad` or the next
    /// free number ([`crate::atomic::aside_names`]), since the first is the one that held the
    /// user's own settings.
    ///
    /// Two cases are left where they are, as [`crate::AppRules::load_from`] leaves them. A
    /// missing file is simply the defaults. A directory in the file's place holds nothing a save
    /// can destroy: a rename cannot replace a directory, so every save fails, loudly, until
    /// someone removes it.
    #[must_use]
    pub fn load_from(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Self::sanitised_default();
            }
            // Not only EISDIR: Windows refuses to open any directory as a file with "access
            // denied", and Unix one this user may not read with EACCES. Both are PermissionDenied,
            // so whether it is a directory is asked of the path itself, on every system.
            Err(err) if err.kind() == std::io::ErrorKind::IsADirectory || path.is_dir() => {
                log::warn!("{}: {err}; using defaults", path.display());
                return Self::sanitised_default();
            }
            Err(err) => {
                Self::move_aside(path, &err);
                return Self::sanitised_default();
            }
        };
        match toml::from_str::<Self>(&text) {
            Ok(mut settings) => {
                settings.sanitise();
                settings
            }
            Err(err) => {
                Self::move_aside(path, &err);
                Self::sanitised_default()
            }
        }
    }

    /// Rename a file that did not load to the first free name [`Settings::next_bad_path`] would
    /// give, saying why in the log.
    ///
    /// When the rename itself fails the file stays where it is, and the log says so. What stops
    /// this rename — a directory this user may not write, a read-only filesystem — stops the next
    /// save's rename as well, so the file is still not replaced.
    fn move_aside(path: &Path, why: &dyn std::fmt::Display) {
        match crate::atomic::rename_to_free_name(
            path,
            crate::atomic::aside_names(Self::bad_path(path)),
        ) {
            Ok(aside) => log::warn!(
                "{}: {why}; moved aside as {} and using defaults",
                path.display(),
                aside.display()
            ),
            Err(rename_err) => log::warn!(
                "{}: {why}; could not move it aside ({rename_err}); using defaults",
                path.display()
            ),
        }
    }

    /// Where [`Settings::load_from`] moves the first file it could not load: `settings.toml.bad`
    /// beside `settings.toml`. A later one goes to `settings.toml.2.bad` and so on.
    #[must_use]
    pub fn bad_path(path: &Path) -> std::path::PathBuf {
        let mut name = path.file_name().map_or_else(
            || std::ffi::OsString::from("settings.toml"),
            ToOwned::to_owned,
        );
        name.push(".bad");
        path.with_file_name(name)
    }

    /// Where [`Settings::load_from`] would move a file that does not load now: the first of
    /// `settings.toml.bad`, `settings.toml.2.bad`, … that nothing has. What `--self-test`, which
    /// moves nothing, tells the user.
    #[must_use]
    pub fn next_bad_path(path: &Path) -> std::path::PathBuf {
        crate::atomic::next_aside_name(Self::bad_path(path))
    }

    /// The defaults as the loader hands them out: sanitised as a file that loaded is, so a first
    /// run and a run that found no file agree with a run that found one.
    fn sanitised_default() -> Self {
        let mut settings = Self::default();
        settings.sanitise();
        settings
    }

    /// Write the settings file, creating the config directory if needed.
    ///
    /// Durable replace (`crate::atomic`): an interrupted save cannot truncate the existing
    /// settings, and the temporary file carries the process id rather than a fixed name, so two
    /// instances saving at once cannot write over each other's half-finished file.
    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&Self::config_path())
    }

    /// [`Settings::save`] to an explicit path — the seam the round-trip tests use.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        // Never write out a value the loader would have to repair; a `nan` in this file is
        // indistinguishable from one the user typed.
        let mut checked = self.clone();
        checked.sanitise();
        let text = toml::to_string_pretty(&checked)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        crate::atomic::write(path, text.as_bytes())
    }

    /// Force the DSP fields into the ranges the controls enforce.
    ///
    /// This file is the one path into the engine with no validation on it: every other route goes
    /// through a slider or a command-line parser, but `settings.toml` is plain TOML, which spells
    /// `nan` and `inf` as ordinary floats and accepts any magnitude. Run on load, so a corrupt or
    /// hand-edited file cannot reach a filter design, and on save, so the file never records a
    /// value the loader would have to fix again.
    pub fn sanitise(&mut self) {
        use crate::limits::{self, finite};

        let default = Self::default();

        // A language written the ISO way or as a locale (`uk`, `ru_RU.UTF-8`) is kept as the table's
        // own code, the one the switch and `--status` show (0.4.0 audit #28). One with no table is
        // left as written: `effective_language` shows the system's language for it.
        if let Some(code) = crate::i18n::canonical_code(&self.language) {
            code.clone_into(&mut self.language);
        }

        // Informational, and dropped rather than clamped when corrupt: see `CalibrationRecord`.
        // A ratio is still held to being a ratio.
        if let Some(record) = &mut self.calibration {
            if record.is_finite() {
                record.clipped_ratio = record.clipped_ratio.clamp(0.0, 1.0);
            } else {
                self.calibration = None;
            }
        }

        self.master_gain = finite(
            self.master_gain,
            limits::MASTER_GAIN_DB,
            default.master_gain,
        );
        self.balance = finite(self.balance, limits::BALANCE_DB, default.balance);
        self.volume_leveling = finite(
            self.volume_leveling,
            limits::VOLUME_LEVELING,
            default.volume_leveling,
        );
        self.filter_q = finite(self.filter_q, limits::FILTER_Q, default.filter_q);
        self.num_bands = self.num_bands.clamp(1, crate::eq::MAX_BANDS as u32);
        // Clamped rather than replaced (audit report #20): the original reads anything outside
        // 10..=120 as 120 (`FxController.cpp:194-198`), so a file asking for 500 got fewer than
        // it asked for, and one asking for 5 got more.
        self.max_user_presets = self
            .max_user_presets
            .clamp(*USER_PRESET_LIMITS.start(), *USER_PRESET_LIMITS.end());

        // Replayed onto the node the user listens through, so an entry that is not a volume
        // anyone set goes, rather than being guessed at. Order is kept: it is the order the
        // devices were first heard on, which is the only history the file has. A second entry
        // for the same lane, device and port can only be a hand edit; the first is the one every
        // lookup answers with, so it is the one kept, and the shadow cannot outlive the next save.
        let mut kept: Vec<TargetVolume> = Vec::with_capacity(self.device_volumes.len());
        for entry in std::mem::take(&mut self.device_volumes)
            .into_iter()
            .filter_map(TargetVolume::sanitised)
        {
            if !kept.iter().any(|seen| seen.same_place(&entry)) {
                kept.push(entry);
            }
        }
        self.device_volumes = kept;
    }

    /// The volume remembered for CrossFX's node while attached to `target` on its port `port`
    /// (empty for a device with no port), if any.
    #[must_use]
    pub fn target_volume(
        &self,
        direction: DeviceDirection,
        target: &str,
        port: &str,
    ) -> Option<&TargetVolume> {
        self.device_volumes
            .iter()
            .find(|entry| entry.is_for(direction, target, port))
    }

    /// Remember a volume the engine reported ([`crate::AudioToUi::TargetVolume`]), replacing the
    /// entry for the same direction, target and port. Returns whether anything changed, so the
    /// caller saves only when there is something to save — a mixer drag reports on every step.
    ///
    /// An entry the loader would drop is not stored: the file never records a value it would
    /// have to discard again. The engine reports no keys of a later version, so an entry it
    /// replaces keeps the ones it had ([`TargetVolume::extra`]).
    pub fn remember_target_volume(&mut self, volume: TargetVolume) -> bool {
        let Some(mut volume) = volume.sanitised() else {
            return false;
        };
        match self
            .device_volumes
            .iter_mut()
            .find(|entry| entry.same_place(&volume))
        {
            Some(entry) => {
                volume.extra.clone_from(&entry.extra);
                if *entry == volume {
                    return false;
                }
                *entry = volume;
                true
            }
            None => {
                self.device_volumes.push(volume);
                true
            }
        }
    }

    /// The `node.name` one lane should attach to on launch. Empty when nothing was ever chosen,
    /// in which case the engine's own device rules pick.
    #[must_use]
    pub fn device_name(&self, direction: DeviceDirection) -> &str {
        match direction {
            DeviceDirection::Output => &self.output_device_name,
            DeviceDirection::Input => &self.input_device_name,
        }
    }

    /// Remember a device choice for one lane. The other lane's memory is kept, and the edit
    /// direction does not move: which chain the window edits is a separate decision
    /// ([`Settings::set_edit_direction`]), and coupling the two was the 0.3.0 bug that made
    /// picking a microphone silently re-point every control at it.
    pub fn set_device_name(&mut self, direction: DeviceDirection, node_name: &str) {
        match direction {
            DeviceDirection::Output => node_name.clone_into(&mut self.output_device_name),
            DeviceDirection::Input => node_name.clone_into(&mut self.input_device_name),
        }
    }

    /// Which chain the window edits. Never touches a device name or a lane's enabled state.
    pub fn set_edit_direction(&mut self, direction: DeviceDirection) {
        self.device_direction = direction;
    }

    /// Whether a lane comes up at start.
    #[must_use]
    pub fn lane_enabled(&self, direction: DeviceDirection) -> bool {
        match direction {
            DeviceDirection::Output => self.output_enabled,
            DeviceDirection::Input => self.input_enabled,
        }
    }

    /// Record whether a lane comes up at start.
    pub fn set_lane_enabled(&mut self, direction: DeviceDirection, enabled: bool) {
        match direction {
            DeviceDirection::Output => self.output_enabled = enabled,
            DeviceDirection::Input => self.input_enabled = enabled,
        }
    }

    /// What the session default was before CrossFX took this direction.
    #[must_use]
    pub fn remembered_default(&self, direction: DeviceDirection) -> &str {
        match direction {
            DeviceDirection::Output => &self.remembered_default_output,
            DeviceDirection::Input => &self.remembered_default_input,
        }
    }

    /// Record what the session default was before CrossFX took this direction.
    pub fn set_remembered_default(&mut self, direction: DeviceDirection, node_name: &str) {
        match direction {
            DeviceDirection::Output => node_name.clone_into(&mut self.remembered_default_output),
            DeviceDirection::Input => node_name.clone_into(&mut self.remembered_default_input),
        }
    }

    /// The preset belonging to one direction.
    #[must_use]
    pub fn preset_for_direction(&self, direction: DeviceDirection) -> &str {
        match direction {
            DeviceDirection::Output => &self.output_preset,
            DeviceDirection::Input => &self.input_preset,
        }
    }

    /// Record the preset for one direction, leaving the other's alone.
    pub fn set_preset_for_direction(&mut self, direction: DeviceDirection, name: &str) {
        match direction {
            DeviceDirection::Output => name.clone_into(&mut self.output_preset),
            DeviceDirection::Input => name.clone_into(&mut self.input_preset),
        }
    }

    /// The preset of the edit direction — what the picker shows on launch.
    #[must_use]
    pub fn selected_preset(&self) -> &str {
        self.preset_for_direction(self.device_direction)
    }

    /// Record the preset for the edit direction, leaving the other direction's alone.
    pub fn set_selected_preset(&mut self, name: &str) {
        self.set_preset_for_direction(self.device_direction, name);
    }

    /// The language code the UI should be shown in right now: the system's, or the explicit
    /// pick when there is one and a translation table exists for it.
    #[must_use]
    pub fn effective_language(&self) -> &'static str {
        crate::i18n::resolve(self.language_follows_system, &self.language)
    }

    /// Record a language pick from Settings. `None` means "follow the system".
    pub fn choose_language(&mut self, code: Option<&str>) {
        match code {
            Some(code) => {
                code.clone_into(&mut self.language);
                self.language_follows_system = false;
            }
            None => self.language_follows_system = true,
        }
    }

    /// The preset remembered for a device, if this device has been seen before in this
    /// direction.
    ///
    /// Keyed on the direction as well as the name: the two lanes' presets come from different
    /// stores, so a memory of the output lane's `General` must never answer for a microphone.
    ///
    /// An entry with an empty preset remembers none: the device priority list adds every device
    /// it sees, whether or not a preset was ever used with it (the original's
    /// `DeviceConfig.cpp:78-85`,
    /// read back with `preset.isNotEmpty()` at `FxController.cpp:1126`).
    #[must_use]
    pub fn preset_for_device(&self, node_name: &str, direction: DeviceDirection) -> Option<&str> {
        self.device_configs
            .iter()
            .find(|c| c.device_id == node_name && c.direction == direction)
            .map(|c| c.preset.as_str())
            .filter(|preset| !preset.is_empty())
    }

    /// Remember which preset was in use for a device.
    pub fn remember_device_preset(
        &mut self,
        node_name: &str,
        description: &str,
        preset: &str,
        form_factor: &str,
        direction: DeviceDirection,
    ) {
        if let Some(existing) = self
            .device_configs
            .iter_mut()
            .find(|c| c.device_id == node_name && c.direction == direction)
        {
            existing.device_name = description.to_owned();
            existing.preset = preset.to_owned();
            // A device can change what it looks like: a USB dock's port is `line-level` until its
            // profile says otherwise, and a Bluetooth headset swaps between `headset` and
            // `headphone` with its profile.
            if !form_factor.is_empty() {
                existing.device_form_factor = form_factor.to_owned();
            }
        } else {
            self.device_configs.push(DeviceConfig {
                device_id: node_name.to_owned(),
                direction,
                device_name: description.to_owned(),
                preset: preset.to_owned(),
                device_form_factor: form_factor.to_owned(),
                extra: toml::Table::new(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_through_toml() {
        let original = Settings::default();
        let text = toml::to_string_pretty(&original).expect("serialise");
        let parsed: Settings = toml::from_str(&text).expect("parse");
        assert_eq!(original, parsed);
    }

    #[test]
    fn an_empty_file_yields_defaults() {
        let parsed: Settings = toml::from_str("").expect("parse empty");
        assert_eq!(parsed, Settings::default());
    }

    #[test]
    fn a_language_written_the_iso_way_is_kept_as_the_tables_code() {
        // 0.4.0 audit #28: `uk` found no table and showed the system's language.
        for (written, kept) in [
            ("uk", "ua"),
            ("bs", "ba"),
            ("ru_RU.UTF-8", "ru"),
            ("zh-cn", "zh-CN"),
        ] {
            let mut settings: Settings = toml::from_str(&format!(
                "language = \"{written}\"\nlanguage_follows_system = false\n"
            ))
            .expect("parse");
            settings.sanitise();
            assert_eq!(settings.language, kept, "{written}");
            assert_eq!(settings.effective_language(), kept, "{written}");
        }
        let mut unknown: Settings = toml::from_str("language = \"hu\"\n").expect("parse");
        unknown.sanitise();
        assert_eq!(
            unknown.language, "hu",
            "a code with no table is left as written"
        );
    }

    #[test]
    fn unknown_keys_do_not_break_loading() {
        let mut parsed: Settings =
            toml::from_str("output_preset = \"Jazz\"\nsome_future_key = 3\n").expect("parse");
        parsed.sanitise();
        assert_eq!(parsed.output_preset, "Jazz");
        assert_eq!(parsed.max_user_presets, 120);
        assert_eq!(
            parsed.extra.get("some_future_key"),
            Some(&toml::Value::Integer(3)),
            "kept to be written back"
        );
    }

    #[test]
    fn a_user_preset_limit_out_of_range_is_clamped_rather_than_reset_to_120() {
        // Audit report #20: the original read anything outside 10..=120 as 120, so 500 became 120
        // and 5 became 120 too.
        for (written, read) in [
            (500, 500),
            (5000, 1000),
            (5, 10),
            (0, 10),
            (10, 10),
            (1000, 1000),
        ] {
            let mut settings: Settings =
                toml::from_str(&format!("max_user_presets = {written}")).expect("parse");
            settings.sanitise();
            assert_eq!(settings.max_user_presets, read, "{written}");
        }
    }

    #[test]
    fn each_direction_remembers_the_default_it_displaced() {
        // The memory that has to outlive the process. A SIGKILL, an OOM kill and a power cut all
        // run no signal handler, so what they leave behind is a session default naming CrossFX's
        // node, which is gone — and the only place to repair that is the next start.
        let mut s = Settings::default();
        assert_eq!(s.remembered_default(crate::DeviceDirection::Output), "");

        s.set_remembered_default(
            crate::DeviceDirection::Output,
            "alsa_output.pci-0000_00_1f.3",
        );
        s.set_remembered_default(crate::DeviceDirection::Input, "alsa_input.usb-fifine");
        assert_eq!(
            s.remembered_default(crate::DeviceDirection::Output),
            "alsa_output.pci-0000_00_1f.3"
        );
        assert_eq!(
            s.remembered_default(crate::DeviceDirection::Input),
            "alsa_input.usb-fifine"
        );

        // And it survives the file, which is the whole point.
        let text = toml::to_string_pretty(&s).expect("serialise");
        let mut back: Settings = toml::from_str(&text).expect("parse");
        back.sanitise();
        assert_eq!(
            back.remembered_default(crate::DeviceDirection::Output),
            "alsa_output.pci-0000_00_1f.3"
        );
    }

    #[test]
    fn the_two_directions_remember_their_own_presets() {
        let mut s = Settings::default();
        s.set_device_name(DeviceDirection::Output, "alsa_output.pci");
        s.set_edit_direction(DeviceDirection::Output);
        s.set_selected_preset("Rock");
        assert_eq!(s.selected_preset(), "Rock");

        s.set_device_name(DeviceDirection::Input, "alsa_input.usb-fifine");
        s.set_edit_direction(DeviceDirection::Input);
        assert_eq!(
            s.selected_preset(),
            "Clean Voice",
            "a microphone must not inherit the music preset"
        );
        s.set_selected_preset("Clean Voice");

        // And switching back returns what was there, in both senses.
        s.set_edit_direction(DeviceDirection::Output);
        assert_eq!(s.selected_preset(), "Rock");
        assert_eq!(
            s.preset_for_direction(DeviceDirection::Input),
            "Clean Voice"
        );
        assert_eq!(s.device_name(DeviceDirection::Output), "alsa_output.pci");
        assert_eq!(
            s.device_name(DeviceDirection::Input),
            "alsa_input.usb-fifine"
        );
    }

    #[test]
    fn choosing_a_device_does_not_move_the_edit_direction() {
        // Coupling the two re-pointed every control at a microphone the moment it was picked
        // (the 0.3.0 bug): choosing a device leaves the edit direction where it was.
        let mut s = Settings::default();
        s.set_device_name(DeviceDirection::Input, "alsa_input.usb-fifine");
        assert_eq!(s.device_direction, DeviceDirection::Output);
        assert_eq!(
            s.device_name(DeviceDirection::Input),
            "alsa_input.usb-fifine"
        );
        s.set_device_name(DeviceDirection::Input, "alsa_input.other");
        assert_eq!(s.device_direction, DeviceDirection::Output);

        s.set_edit_direction(DeviceDirection::Input);
        assert_eq!(s.device_direction, DeviceDirection::Input);
        // And the edit direction moves nothing else.
        assert_eq!(s.device_name(DeviceDirection::Input), "alsa_input.other");
        assert_eq!(s.device_name(DeviceDirection::Output), "");
        assert!(s.output_enabled);
        assert!(!s.input_enabled);
    }

    #[test]
    fn device_presets_are_remembered_and_updated() {
        let mut s = Settings::default();
        s.remember_device_preset(
            "alsa_output.pci-0000_00_1f.3",
            "Speakers",
            "Rock",
            "speaker",
            DeviceDirection::Output,
        );
        assert_eq!(
            s.preset_for_device("alsa_output.pci-0000_00_1f.3", DeviceDirection::Output),
            Some("Rock")
        );
        s.remember_device_preset(
            "alsa_output.pci-0000_00_1f.3",
            "Speakers",
            "Jazz",
            "headphone",
            DeviceDirection::Output,
        );
        assert_eq!(s.device_configs.len(), 1);
        assert_eq!(
            s.preset_for_device("alsa_output.pci-0000_00_1f.3", DeviceDirection::Output),
            Some("Jazz")
        );
        assert_eq!(
            s.device_configs[0].device_form_factor, "headphone",
            "a device may change what it looks like"
        );
    }

    #[test]
    fn a_device_memory_belongs_to_one_direction() {
        // The two lanes' presets come from different stores and their names can collide, so the
        // same node name in the two directions is two memories, and one never answers for the
        // other.
        let mut s = Settings::default();
        s.remember_device_preset("dock", "Dock", "Rock", "", DeviceDirection::Output);
        s.remember_device_preset("dock", "Dock", "Headset", "", DeviceDirection::Input);
        assert_eq!(s.device_configs.len(), 2);
        assert_eq!(
            s.preset_for_device("dock", DeviceDirection::Output),
            Some("Rock")
        );
        assert_eq!(
            s.preset_for_device("dock", DeviceDirection::Input),
            Some("Headset")
        );
        assert_eq!(
            s.preset_for_device("alsa_input.usb-fifine", DeviceDirection::Input),
            None
        );
        // Updating one leaves the other alone.
        s.remember_device_preset("dock", "Dock", "Jazz", "", DeviceDirection::Output);
        assert_eq!(s.device_configs.len(), 2);
        assert_eq!(
            s.preset_for_device("dock", DeviceDirection::Input),
            Some("Headset")
        );
    }

    #[test]
    fn a_device_the_priority_list_knows_without_a_preset_remembers_none() {
        let mut s = Settings::default();
        s.device_configs.push(DeviceConfig {
            device_id: "alsa_output.hdmi".to_owned(),
            direction: DeviceDirection::Output,
            device_name: "HDMI".to_owned(),
            ..DeviceConfig::default()
        });
        assert_eq!(
            s.preset_for_device("alsa_output.hdmi", DeviceDirection::Output),
            None
        );
        // Once a preset is used with it, it remembers that one, in the same entry.
        s.remember_device_preset(
            "alsa_output.hdmi",
            "HDMI",
            "Rock",
            "",
            DeviceDirection::Output,
        );
        assert_eq!(s.device_configs.len(), 1);
        assert_eq!(
            s.preset_for_device("alsa_output.hdmi", DeviceDirection::Output),
            Some("Rock")
        );
    }

    #[test]
    fn a_device_config_without_a_direction_reads_as_an_output() {
        // An entry a hand edit left without the key costs that key, not the entry: it is an
        // output's, the default.
        let parsed: Settings = toml::from_str(
            "[[device_configs]]\ndevice_id = \"alsa_output.pci\"\ndevice_name = \"Speakers\"\npreset = \"Rock\"\ndevice_form_factor = \"speaker\"\n",
        )
        .expect("parse");
        assert_eq!(parsed.device_configs.len(), 1);
        assert_eq!(parsed.device_configs[0].direction, DeviceDirection::Output);
        assert_eq!(
            parsed.preset_for_device("alsa_output.pci", DeviceDirection::Output),
            Some("Rock")
        );
        assert_eq!(
            parsed.preset_for_device("alsa_output.pci", DeviceDirection::Input),
            None
        );
        // And an entry that says which lane it is for is believed.
        let parsed: Settings = toml::from_str(
            "[[device_configs]]\ndevice_id = \"alsa_input.usb\"\ndirection = \"input\"\npreset = \"Headset\"\n",
        )
        .expect("parse");
        assert_eq!(parsed.device_configs[0].direction, DeviceDirection::Input);
    }

    #[test]
    fn a_hand_edited_file_cannot_put_a_non_finite_value_into_the_dsp() {
        // TOML spells these as ordinary floats, and this file is the only route into the engine
        // with no slider or argument parser in front of it.
        let text = "\
master_gain = nan
balance = inf
volume_leveling = -inf
filter_q = nan
num_bands = 4000000
";
        let mut parsed: Settings = toml::from_str(text).expect("TOML accepts nan and inf");
        assert!(
            parsed.master_gain.is_nan(),
            "the hazard is real before sanitising"
        );

        parsed.sanitise();
        let default = Settings::default();
        // Non-finite falls back to the default rather than clamping: +inf on the master gain
        // would otherwise mean "maximum boost" to a user whose file was merely corrupted.
        assert_eq!(parsed.master_gain, default.master_gain);
        assert_eq!(parsed.balance, default.balance);
        assert_eq!(parsed.volume_leveling, default.volume_leveling);
        assert_eq!(parsed.filter_q, default.filter_q);
        assert_eq!(parsed.num_bands, crate::eq::MAX_BANDS as u32);
    }

    #[test]
    fn whether_a_lane_runs_is_its_own_key_whatever_the_edit_direction_says() {
        // The user detached the microphone while still editing its chain.
        let text = "device_direction = \"input\"\ninput_enabled = false\n";
        let mut parsed: Settings = toml::from_str(text).expect("parse");
        parsed.sanitise();
        assert!(!parsed.input_enabled);
        assert!(!parsed.lane_enabled(DeviceDirection::Input));
        // And the converse: the lane on while the window edits the speakers.
        let text = "device_direction = \"output\"\ninput_enabled = true\noutput_enabled = false\n";
        let mut parsed: Settings = toml::from_str(text).expect("parse");
        parsed.sanitise();
        assert!(parsed.input_enabled);
        assert!(!parsed.output_enabled);
        assert!(!parsed.lane_enabled(DeviceDirection::Output));
        // A file without the key has the input lane off, as a fresh install does, the edit
        // direction notwithstanding; the microphone it remembers stays remembered for when the
        // lane is switched on.
        let text = "device_direction = \"input\"\ninput_device_name = \"alsa_input.usb-fifine\"\n";
        let mut parsed: Settings = toml::from_str(text).expect("parse");
        parsed.sanitise();
        assert!(!parsed.lane_enabled(DeviceDirection::Input));
        assert!(parsed.lane_enabled(DeviceDirection::Output));
        assert_eq!(parsed.device_direction, DeviceDirection::Input);
        assert_eq!(
            parsed.device_name(DeviceDirection::Input),
            "alsa_input.usb-fifine"
        );
    }

    #[test]
    fn the_lane_switches_are_written_and_read_back() {
        let mut s = Settings::default();
        s.set_lane_enabled(DeviceDirection::Output, false);
        s.set_lane_enabled(DeviceDirection::Input, true);
        assert!(!s.lane_enabled(DeviceDirection::Output));
        assert!(s.lane_enabled(DeviceDirection::Input));
        let text = toml::to_string_pretty(&s).expect("serialise");
        assert!(text.contains("input_enabled = true"), "{text}");
        assert!(text.contains("output_enabled = false"), "{text}");
        let back: Settings = toml::from_str(&text).expect("parse");
        assert!(back.input_enabled);
        assert!(!back.output_enabled);
    }

    #[test]
    fn a_file_that_does_not_parse_is_moved_aside_rather_than_silently_reset() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.toml");
        let broken = "power = \"yes\"\noutput_preset = \"Rock\"\n";
        std::fs::write(&path, broken).expect("write");

        let loaded = Settings::load_from(&path);
        assert_eq!(loaded, {
            let mut d = Settings::default();
            d.sanitise();
            d
        });

        let aside = dir.path().join("settings.toml.bad");
        assert_eq!(Settings::bad_path(&path), aside);
        assert_eq!(
            std::fs::read_to_string(&aside).expect("the broken file was moved aside"),
            broken,
            "byte for byte, so the hand edit can be read back"
        );
        assert!(
            !path.exists(),
            "and nothing is left to be overwritten in place"
        );

        // The next save writes a fresh file beside it and leaves the evidence alone.
        loaded.save_to(&path).expect("save");
        assert!(path.exists());
        assert_eq!(
            std::fs::read_to_string(&aside).expect("still there"),
            broken
        );
    }

    /// What [`Settings::load_from`] hands out for a file it could not use.
    fn loaded_defaults() -> Settings {
        let mut defaults = Settings::default();
        defaults.sanitise();
        defaults
    }

    #[test]
    fn a_settings_file_that_is_not_utf8_is_moved_aside_rather_than_replaced_by_the_defaults() {
        // A comment saved in CP1251 by an editor on a legacy encoding: `Звук` is four bytes there
        // and none of them UTF-8. The file used to load as the defaults and stay where it was,
        // and the next save renamed the defaults over it.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.toml");
        let cp1251: &[u8] = b"# \xc7\xe2\xf3\xea\npower = false\noutput_preset = \"Rock\"\n";
        std::fs::write(&path, cp1251).expect("write");

        assert_eq!(Settings::load_from(&path), loaded_defaults());
        let aside = Settings::bad_path(&path);
        assert_eq!(
            std::fs::read(&aside).expect("the file was moved aside"),
            cp1251,
            "byte for byte, so it can be saved again in UTF-8 and put back"
        );
        assert!(!path.exists(), "nothing is left to be overwritten in place");

        // The save a slider or the way out makes cannot touch it now.
        loaded_defaults().save_to(&path).expect("save");
        assert_eq!(std::fs::read(&aside).expect("still there"), cp1251);
    }

    #[test]
    fn a_settings_file_this_user_may_not_read_is_moved_aside_rather_than_replaced() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.toml");
        let text = "output_preset = \"Jazz\"\n";
        std::fs::write(&path, text).expect("write");
        assert!(crate::atomic::tests::forbid_reading(&path), "chmod");
        if std::fs::read(&path).is_ok() {
            // Root reads a mode-000 file anyway, so there is no unreadable file to test with.
            return;
        }

        assert_eq!(Settings::load_from(&path), loaded_defaults());
        let aside = Settings::bad_path(&path);
        assert!(
            !path.exists(),
            "nothing is left for the next save to replace"
        );

        loaded_defaults().save_to(&path).expect("save");
        assert!(crate::atomic::tests::allow_reading(&aside), "chmod");
        assert_eq!(
            std::fs::read_to_string(&aside).expect("readable once allowed"),
            text,
            "the user's preset is still there once the permissions are fixed"
        );
    }

    #[test]
    fn a_directory_in_the_settings_files_place_is_left_where_it_is() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.toml");
        std::fs::create_dir(&path).expect("create");
        std::fs::write(path.join("notes"), "mine").expect("write");

        assert_eq!(Settings::load_from(&path), loaded_defaults());
        assert!(path.is_dir());
        assert!(!Settings::bad_path(&path).exists());
        // And no save can destroy it: a rename cannot replace a directory.
        assert!(loaded_defaults().save_to(&path).is_err());
        assert_eq!(
            std::fs::read_to_string(path.join("notes")).expect("still there"),
            "mine"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_this_user_may_not_read_in_the_settings_files_place_is_left_where_it_is() {
        // Opening a directory this user may not read fails with EACCES, not EISDIR: 0.5.0 took
        // that for an unreadable file and moved the directory aside as `settings.toml.bad`. It is
        // a directory all the same, and is left where it is, as a readable one is.
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.toml");
        let aside = Settings::bad_path(&path);
        std::fs::create_dir(&path).expect("create");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).expect("chmod");
        if std::fs::read_dir(&path).is_ok() {
            // Root reads a mode-000 directory anyway, which is the readable case again.
            eprintln!("note: {} is readable to this user", path.display());
        }

        let loaded = Settings::load_from(&path);
        let left = path.is_dir();
        let moved = aside.exists();
        // Searchable again wherever it is, or the scratch directory cannot be removed.
        for place in [&path, &aside] {
            let _ = std::fs::set_permissions(place, std::fs::Permissions::from_mode(0o700));
        }
        assert_eq!(loaded, loaded_defaults());
        assert!(left, "the directory is still in the file's place");
        assert!(!moved, "and was not moved aside");
    }

    #[test]
    fn a_second_broken_settings_file_is_moved_aside_beside_the_first_never_over_it() {
        // The first `.bad` is the one with the user's own settings in it; a later file that fails
        // to load — a newer CrossFX's value this one does not know, after a downgrade — is
        // usually the defaults with one bad line, and must not take its place.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.toml");
        let precious = "power = \"yes\"\noutput_preset = \"MyPrecious\"\n";
        std::fs::write(&path, precious).expect("write");
        assert_eq!(Settings::load_from(&path), loaded_defaults());
        loaded_defaults().save_to(&path).expect("save");

        std::fs::write(&path, "power = \"no\"\n").expect("write");
        assert_eq!(
            Settings::next_bad_path(&path),
            dir.path().join("settings.toml.2.bad"),
            "what the self-test would say"
        );
        assert_eq!(Settings::load_from(&path), loaded_defaults());

        assert_eq!(
            std::fs::read_to_string(dir.path().join("settings.toml.bad")).expect("first"),
            precious
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("settings.toml.2.bad")).expect("second"),
            "power = \"no\"\n"
        );
        assert!(!path.exists());
    }

    #[test]
    fn a_settings_file_that_is_a_symbolic_link_stays_one_after_a_save() {
        // GNU Stow and chezmoi's symlink mode keep the file as a link into the user's dotfiles;
        // a save used to replace the link with a plain file, and every change after it was
        // missing from the dotfiles.
        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().join("dotfiles-settings.toml");
        let link = dir.path().join("settings.toml");
        let mut original = loaded_defaults();
        original.output_preset = "Rock".into();
        original.save_to(&real).expect("save the dotfiles copy");
        let Some(()) = crate::atomic::tests::symlink(&real, &link) else {
            return;
        };

        let mut settings = Settings::load_from(&link);
        assert_eq!(settings.output_preset, "Rock");
        settings.output_preset = "Jazz".into();
        settings.save_to(&link).expect("save");

        assert!(
            std::fs::symlink_metadata(&link)
                .expect("there")
                .file_type()
                .is_symlink(),
            "still a link"
        );
        assert_eq!(Settings::load_from(&real).output_preset, "Jazz");
    }

    #[test]
    fn a_missing_file_yields_sanitised_defaults_and_creates_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.toml");
        let loaded = Settings::load_from(&path);
        assert_eq!(
            loaded,
            Settings::sanitised_default(),
            "the loader's defaults, sanitised"
        );
        assert!(!loaded.input_enabled);
        assert_eq!(loaded.output_preset, "General");
        assert!(!path.exists());
        assert!(!Settings::bad_path(&path).exists());
    }

    #[test]
    fn every_new_key_survives_a_round_trip_through_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("settings.toml");
        let original = Settings {
            input_enabled: true,
            output_enabled: false,
            device_direction: DeviceDirection::Input,
            noise_suppression: NoiseSuppressionOverride::Strong,
            denoise_channels: DenoiseChannelsOverride::Linked,
            deesser_mode: DeEsserMode::Adaptive,
            echo_cancel: true,
            dereverb: DereverbLevel::Medium,
            calibration: Some(CalibrationRecord {
                noise_floor_db: -52.5,
                speech_rms_db: -21.0,
                speech_peak_db: -6.5,
                clipped_ratio: 0.001,
                unix_time: 1_760_000_000,
                preset: "Calibrated — Fifine K669".to_owned(),
                device: "alsa_input.usb-fifine".to_owned(),
                extra: toml::Table::new(),
            }),
            device_configs: vec![DeviceConfig {
                device_id: "alsa_input.usb-fifine".to_owned(),
                direction: DeviceDirection::Input,
                device_name: "Fifine K669".to_owned(),
                preset: "Calibrated — Fifine K669".to_owned(),
                device_form_factor: "microphone".to_owned(),
                extra: toml::Table::new(),
            }],
            ..Settings::default()
        };
        original.save_to(&path).expect("save creates the directory");
        let back = Settings::load_from(&path);
        assert_eq!(back, original);

        // And the keys are spelled the way the design record and the CLI spell them.
        let text = std::fs::read_to_string(&path).expect("read");
        for line in [
            "noise_suppression = \"strong\"",
            "denoise_channels = \"linked\"",
            "deesser_mode = \"adaptive\"",
            "echo_cancel = true",
            "dereverb = \"medium\"",
            "input_enabled = true",
            "output_enabled = false",
            "device_direction = \"input\"",
            "direction = \"input\"",
            "[calibration]",
        ] {
            assert!(text.contains(line), "missing {line:?} in:\n{text}");
        }
    }

    #[test]
    fn a_0_4_0_file_is_still_readable_by_the_keys_alone() {
        // Spelled by hand, the way the design record spells them, so a rename of a variant or a
        // change of `rename_all` cannot pass the round-trip test and still break the file.
        let text = "\
noise_suppression = \"light\"
denoise_channels = \"mono\"
deesser_mode = \"adaptive\"
echo_cancel = true
dereverb = \"strong\"

[calibration]
noise_floor_db = -48.0
speech_rms_db = -20.0
speech_peak_db = -4.0
clipped_ratio = 0.0
unix_time = 1
preset = \"Calibrated — Mic\"
device = \"mic\"
";
        let mut parsed: Settings = toml::from_str(text).expect("parse");
        parsed.sanitise();
        assert_eq!(parsed.noise_suppression, NoiseSuppressionOverride::Light);
        assert_eq!(parsed.denoise_channels, DenoiseChannelsOverride::Mono);
        assert_eq!(parsed.deesser_mode, DeEsserMode::Adaptive);
        assert!(parsed.echo_cancel);
        assert_eq!(parsed.dereverb, DereverbLevel::Strong);
        let record = parsed.calibration.expect("record");
        assert_eq!(record.noise_floor_db, -48.0);
        assert_eq!(record.preset, "Calibrated — Mic");
        assert_eq!(record.device, "mic");
        // A partial record fills in the rest rather than failing the whole file.
        let partial: Settings =
            toml::from_str("[calibration]\nnoise_floor_db = -40.0\n").expect("parse");
        assert_eq!(
            partial.calibration,
            Some(CalibrationRecord {
                noise_floor_db: -40.0,
                ..CalibrationRecord::default()
            })
        );
    }

    #[test]
    fn a_corrupt_calibration_record_is_dropped_and_a_ratio_stays_a_ratio() {
        let mut nan: Settings =
            toml::from_str("[calibration]\nnoise_floor_db = nan\n").expect("TOML accepts nan");
        assert!(
            nan.calibration.is_some(),
            "the hazard is real before sanitising"
        );
        nan.sanitise();
        assert_eq!(nan.calibration, None, "a floor of NaN is not a measurement");

        let mut inf: Settings =
            toml::from_str("[calibration]\nspeech_rms_db = -inf\n").expect("parse");
        inf.sanitise();
        assert_eq!(inf.calibration, None);

        let mut ratio: Settings =
            toml::from_str("[calibration]\nclipped_ratio = 7.5\n").expect("parse");
        ratio.sanitise();
        assert_eq!(
            ratio.calibration.map(|r| r.clipped_ratio),
            Some(1.0),
            "finite but impossible is clamped, as everywhere else"
        );
        let mut fine: Settings =
            toml::from_str("[calibration]\nclipped_ratio = 0.25\nunix_time = 9\n").expect("parse");
        fine.sanitise();
        assert_eq!(
            fine.calibration.as_ref().map(|r| r.clipped_ratio),
            Some(0.25)
        );
        assert_eq!(fine.calibration.map(|r| r.unix_time), Some(9));
    }

    #[test]
    fn the_microphone_overrides_default_to_following_the_preset() {
        let s = Settings::default();
        assert_eq!(s.noise_suppression, NoiseSuppressionOverride::Preset);
        assert_eq!(s.denoise_channels, DenoiseChannelsOverride::Preset);
        assert_eq!(s.deesser_mode, DeEsserMode::Classic);
        assert!(!s.echo_cancel);
        assert_eq!(s.dereverb, DereverbLevel::Off);
        assert_eq!(s.calibration, None);
        assert!(s.output_enabled);
        // An empty file — a fresh install — agrees.
        let mut fresh: Settings = toml::from_str("").expect("parse");
        fresh.sanitise();
        assert_eq!(fresh.noise_suppression, NoiseSuppressionOverride::Preset);
        assert!(!fresh.input_enabled);
    }

    fn volume(direction: DeviceDirection, target: &str, volumes: &[f32]) -> TargetVolume {
        TargetVolume {
            direction,
            target: target.to_owned(),
            port: String::new(),
            channel_volumes: volumes.to_vec(),
            mute: false,
            extra: toml::Table::new(),
        }
    }

    #[test]
    fn a_fresh_install_remembers_no_volumes_and_follows_the_priority_list() {
        let s = Settings::default();
        assert!(s.device_volumes.is_empty());
        assert!(
            !s.follow_system_default,
            "the original's behaviour: the ranked list picks"
        );
        let mut fresh: Settings = toml::from_str("").expect("parse");
        fresh.sanitise();
        assert!(fresh.device_volumes.is_empty());
        assert!(!fresh.follow_system_default);
    }

    #[test]
    fn remembered_volumes_and_the_follow_switch_survive_a_round_trip_through_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.toml");
        let original = Settings {
            follow_system_default: true,
            device_volumes: vec![
                volume(
                    DeviceDirection::Output,
                    "alsa_output.usb-headphones",
                    &[0.3, 0.3],
                ),
                TargetVolume {
                    mute: true,
                    ..volume(DeviceDirection::Input, "alsa_input.usb-fifine", &[1.5])
                },
            ],
            ..Settings::default()
        };
        original.save_to(&path).expect("save");
        let mut expected = original.clone();
        expected.sanitise();
        assert_eq!(Settings::load_from(&path), expected);

        let text = std::fs::read_to_string(&path).expect("read");
        for line in [
            "follow_system_default = true",
            "[[device_volumes]]",
            "target = \"alsa_output.usb-headphones\"",
            "channel_volumes = [1.5]",
            "direction = \"input\"",
            "mute = true",
        ] {
            assert!(text.contains(line), "missing {line:?} in:\n{text}");
        }
    }

    #[test]
    fn a_hand_edited_volume_that_means_nothing_is_dropped_and_one_too_loud_is_clamped() {
        // TOML spells `nan` as a float, and a remembered volume is replayed onto the node the
        // user listens through; a corrupt one must cost that entry, never the file, and never be
        // guessed at.
        let text = "\
[[device_volumes]]
target = \"nan\"
channel_volumes = [nan, 0.5]

[[device_volumes]]
target = \"negative\"
channel_volumes = [-0.5, 0.5]

[[device_volumes]]
target = \"infinite\"
channel_volumes = [inf]

[[device_volumes]]
target = \"loud\"
channel_volumes = [12.0, 0.5]

[[device_volumes]]
direction = \"input\"
target = \"fine\"
channel_volumes = [0.0, 4.0]
mute = true
";
        let mut parsed: Settings = toml::from_str(text).expect("TOML accepts nan and inf");
        assert_eq!(
            parsed.device_volumes.len(),
            5,
            "the hazard is real before sanitising"
        );
        parsed.sanitise();
        assert_eq!(
            parsed.device_volumes,
            [
                volume(DeviceDirection::Output, "loud", &[4.0, 0.5]),
                TargetVolume {
                    mute: true,
                    ..volume(DeviceDirection::Input, "fine", &[0.0, 4.0])
                },
            ]
        );
    }

    #[test]
    fn a_hand_edited_duplicate_volume_keeps_the_entry_every_lookup_answers_with() {
        let mut s = Settings {
            device_volumes: vec![
                volume(DeviceDirection::Output, "dock", &[0.2, 0.2]),
                volume(DeviceDirection::Input, "dock", &[0.9]),
                volume(DeviceDirection::Output, "dock", &[1.0, 1.0]),
            ],
            ..Settings::default()
        };
        let answered = s
            .target_volume(DeviceDirection::Output, "dock", "")
            .cloned()
            .expect("remembered");
        s.sanitise();
        assert_eq!(
            s.device_volumes.len(),
            2,
            "the same name in the other lane stays"
        );
        assert_eq!(
            s.target_volume(DeviceDirection::Output, "dock", ""),
            Some(&answered)
        );
        assert_eq!(answered.channel_volumes, [0.2, 0.2]);
    }

    fn on_port(port: &str, entry: TargetVolume) -> TargetVolume {
        TargetVolume {
            port: port.to_owned(),
            ..entry
        }
    }

    #[test]
    fn the_speakers_and_the_headphones_of_one_sink_keep_a_volume_each() {
        const SINK: &str = "alsa_output.pci-0000_00_1f.3.analog-stereo";
        let mut s = Settings::default();
        let speakers = on_port(
            "analog-output-speaker",
            volume(DeviceDirection::Output, SINK, &[1.0, 1.0]),
        );
        let headphones = on_port(
            "analog-output-headphones",
            volume(DeviceDirection::Output, SINK, &[0.2, 0.2]),
        );
        assert!(s.remember_target_volume(speakers.clone()));
        assert!(s.remember_target_volume(headphones.clone()));
        assert_eq!(
            s.device_volumes.len(),
            2,
            "one node, two ports, two entries"
        );
        assert_eq!(
            s.target_volume(DeviceDirection::Output, SINK, "analog-output-headphones"),
            Some(&headphones)
        );
        assert_eq!(
            s.target_volume(DeviceDirection::Output, SINK, "analog-output-speaker"),
            Some(&speakers)
        );
        assert_eq!(
            s.target_volume(DeviceDirection::Output, SINK, ""),
            None,
            "the node with no port named is a third place"
        );
        assert!(s.remember_target_volume(on_port(
            "analog-output-headphones",
            volume(DeviceDirection::Output, SINK, &[0.3, 0.3]),
        )));
        assert_eq!(s.device_volumes.len(), 2, "replaced on its own port");
        assert_eq!(
            s.target_volume(DeviceDirection::Output, SINK, "analog-output-speaker"),
            Some(&speakers),
            "and the other port's is untouched"
        );
    }

    #[test]
    fn a_port_is_written_only_when_there_is_one_and_an_entry_without_one_reads_as_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.toml");
        let original = Settings {
            device_volumes: vec![
                on_port(
                    "analog-output-headphones",
                    volume(DeviceDirection::Output, "alsa_output.pci", &[0.2, 0.2]),
                ),
                volume(DeviceDirection::Output, "bluez_output.x.1", &[0.5, 0.5]),
            ],
            ..Settings::default()
        };
        original.save_to(&path).expect("save");
        let text = std::fs::read_to_string(&path).expect("read");
        assert_eq!(
            text.matches("port = ").count(),
            1,
            "only the entry with a port says so:\n{text}"
        );
        assert!(text.contains("port = \"analog-output-headphones\""));
        let mut expected = original.clone();
        expected.sanitise();
        assert_eq!(Settings::load_from(&path), expected);

        // A file written before ports were kept.
        let parsed: Settings = toml::from_str(
            "[[device_volumes]]\ntarget = \"alsa_output.pci\"\nchannel_volumes = [0.4, 0.4]\n",
        )
        .expect("an entry without a port");
        assert_eq!(parsed.device_volumes[0].port, "");
    }

    #[test]
    fn a_hand_edited_duplicate_on_another_port_is_no_duplicate() {
        let mut s = Settings {
            device_volumes: vec![
                on_port(
                    "speaker",
                    volume(DeviceDirection::Output, "hda", &[0.9, 0.9]),
                ),
                on_port(
                    "headphones",
                    volume(DeviceDirection::Output, "hda", &[0.1, 0.1]),
                ),
                on_port(
                    "speaker",
                    volume(DeviceDirection::Output, "hda", &[0.5, 0.5]),
                ),
            ],
            ..Settings::default()
        };
        s.sanitise();
        assert_eq!(s.device_volumes.len(), 2);
        assert_eq!(
            s.target_volume(DeviceDirection::Output, "hda", "speaker")
                .map(|entry| entry.channel_volumes.clone()),
            Some(vec![0.9, 0.9])
        );
    }

    #[test]
    fn a_reported_volume_replaces_the_one_for_the_same_lane_and_device_only() {
        let mut s = Settings::default();
        assert!(s.remember_target_volume(volume(DeviceDirection::Output, "dock", &[0.5, 0.5])));
        assert!(s.remember_target_volume(volume(DeviceDirection::Input, "dock", &[0.8])));
        assert!(s.remember_target_volume(volume(DeviceDirection::Output, "hdmi", &[1.0, 1.0])));
        assert_eq!(s.device_volumes.len(), 3);

        assert!(s.remember_target_volume(volume(DeviceDirection::Output, "dock", &[0.25, 0.25])));
        assert_eq!(s.device_volumes.len(), 3, "replaced in place, not appended");
        assert_eq!(
            s.target_volume(DeviceDirection::Output, "dock", "")
                .map(|v| v.channel_volumes.clone()),
            Some(vec![0.25, 0.25])
        );
        assert_eq!(
            s.target_volume(DeviceDirection::Input, "dock", "")
                .map(|v| v.channel_volumes.clone()),
            Some(vec![0.8]),
            "the microphone's memory of the same node name is its own"
        );
        assert_eq!(s.target_volume(DeviceDirection::Input, "hdmi", ""), None);

        // The same report twice is not a change, so a mixer drag that settles saves once.
        assert!(!s.remember_target_volume(volume(DeviceDirection::Output, "dock", &[0.25, 0.25])));
        // A mute is a change on its own.
        assert!(s.remember_target_volume(TargetVolume {
            mute: true,
            ..volume(DeviceDirection::Output, "dock", &[0.25, 0.25])
        }));
    }

    #[test]
    fn a_reported_volume_the_loader_would_drop_is_never_stored() {
        let mut s = Settings::default();
        assert!(s.remember_target_volume(volume(DeviceDirection::Output, "dock", &[0.5, 0.5])));
        assert!(!s.remember_target_volume(volume(DeviceDirection::Output, "dock", &[f32::NAN])));
        assert!(!s.remember_target_volume(volume(DeviceDirection::Output, "", &[0.5])));
        assert_eq!(
            s.target_volume(DeviceDirection::Output, "dock", "")
                .map(|v| v.channel_volumes.clone()),
            Some(vec![0.5, 0.5]),
            "the good memory is not overwritten by a bad report"
        );
        // A report too loud to replay is stored as the loudest level that can be.
        assert!(s.remember_target_volume(volume(DeviceDirection::Output, "dock", &[7.0, 7.0])));
        assert_eq!(
            s.target_volume(DeviceDirection::Output, "dock", "")
                .map(|v| v.channel_volumes.clone()),
            Some(vec![4.0, 4.0])
        );
    }

    #[test]
    fn out_of_range_values_are_pulled_back_to_what_the_controls_allow() {
        let mut s = Settings {
            master_gain: 500.0,
            balance: -99.0,
            volume_leveling: 12.0,
            filter_q: 0.1,
            num_bands: 0,
            ..Settings::default()
        };
        s.sanitise();
        assert_eq!(s.master_gain, 20.0);
        assert_eq!(s.balance, -20.0);
        assert_eq!(s.volume_leveling, 4.0);
        assert_eq!(s.filter_q, 1.0);
        assert_eq!(s.num_bands, 1);
    }

    #[test]
    fn the_windows_parity_level_is_written_only_when_it_is_not_off() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let path = dir.path().join("settings.toml");
        Settings::default().save_to(&path).expect("save");
        let written = std::fs::read_to_string(&path).expect("read back");
        assert!(!written.contains("windows_parity"), "{written}");

        for level in [
            WindowsParity::Interface,
            WindowsParity::Sound,
            WindowsParity::Full,
        ] {
            let settings = Settings {
                windows_parity: level,
                ..Settings::default()
            };
            settings.save_to(&path).expect("save");
            let written = std::fs::read_to_string(&path).expect("read back");
            assert!(
                written.contains(&format!("windows_parity = \"{}\"", level.key())),
                "{written}"
            );
            assert_eq!(Settings::load_from(&path).windows_parity, level);
        }
    }

    #[test]
    fn the_unshifted_export_is_written_only_when_it_is_chosen() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let path = dir.path().join("settings.toml");
        Settings::default().save_to(&path).expect("save");
        let written = std::fs::read_to_string(&path).expect("read back");
        assert!(!written.contains("export_unshifted"), "{written}");
        assert!(!Settings::load_from(&path).export_unshifted);

        Settings {
            export_unshifted: true,
            ..Settings::default()
        }
        .save_to(&path)
        .expect("save");
        let written = std::fs::read_to_string(&path).expect("read back");
        assert!(written.contains("export_unshifted = true"), "{written}");
        assert!(Settings::load_from(&path).export_unshifted);
    }

    #[test]
    fn a_file_that_says_full_survives_a_load_and_a_save_byte_for_byte_while_it_runs_as_sound() {
        // 0.6.0 writes Everything; 0.5.0 runs it as Interface and sound and writes it back as it
        // was, so 0.6.0 -> 0.5.0 -> 0.6.0 finds Everything again.
        let dir = tempfile::tempdir().expect("a scratch directory");
        let path = dir.path().join("settings.toml");
        Settings {
            windows_parity: WindowsParity::Full,
            output_preset: "Rock".into(),
            ..Settings::default()
        }
        .save_to(&path)
        .expect("save");
        let written = std::fs::read(&path).expect("read back");
        assert!(
            String::from_utf8_lossy(&written).contains("windows_parity = \"full\""),
            "{}",
            String::from_utf8_lossy(&written)
        );

        let loaded = Settings::load_from(&path);
        assert_eq!(loaded.windows_parity, WindowsParity::Full);
        assert_eq!(
            loaded.windows_parity.offered_or_below(),
            WindowsParity::Sound
        );
        loaded.save_to(&path).expect("save again");
        assert_eq!(std::fs::read(&path).expect("read again"), written);
    }

    #[test]
    fn a_windows_parity_value_that_cannot_be_read_costs_that_key_and_never_the_file() {
        // A10: a boolean, an unknown word, a number, a table and an array are all read as a level,
        // and the rest of the file is believed; nothing is moved aside.
        for (value, level) in [
            ("true", WindowsParity::Sound),
            ("false", WindowsParity::Off),
            ("\"EVERYTHING\"", WindowsParity::Full),
            ("\"full\"", WindowsParity::Full),
            ("\"Interface\"", WindowsParity::Interface),
            ("\"like windows\"", WindowsParity::Off),
            ("3", WindowsParity::Off),
            ("-0.5", WindowsParity::Off),
            ("{ level = \"full\" }", WindowsParity::Off),
            ("[\"sound\"]", WindowsParity::Off),
        ] {
            let dir = tempfile::tempdir().expect("a scratch directory");
            let path = dir.path().join("settings.toml");
            std::fs::write(
                &path,
                format!("power = false\nwindows_parity = {value}\noutput_preset = \"Rock\"\n"),
            )
            .expect("write");
            let loaded = Settings::load_from(&path);
            assert!(
                !Settings::bad_path(&path).exists(),
                "{value} moved the file"
            );
            assert_eq!(loaded.windows_parity, level, "{value}");
            assert!(!loaded.power, "{value}");
            assert_eq!(loaded.output_preset, "Rock", "{value}");
            assert!(loaded.extra.is_empty(), "{value}: {:?}", loaded.extra);
        }
        // A table header of that name, rather than an inline table, too.
        let dir = tempfile::tempdir().expect("a scratch directory");
        let path = dir.path().join("settings.toml");
        std::fs::write(
            &path,
            "power = false\n\n[windows_parity]\nlevel = \"full\"\n",
        )
        .expect("write");
        let loaded = Settings::load_from(&path);
        assert!(!Settings::bad_path(&path).exists());
        assert_eq!(loaded.windows_parity, WindowsParity::Off);
        assert!(!loaded.power);
    }

    /// A settings file as a later version might write it: this version's keys, and some it has
    /// never heard of — a plain value, an array, a table and an array of tables at the top, and a
    /// key inside an entry of each array of tables this version writes and inside `[calibration]`.
    fn with_later_keys() -> Settings {
        let nested = |text: &str| -> toml::Table { toml::from_str(text).expect("a later key") };
        let later: toml::Table = toml::from_str(
            "\
theme = \"studio-light\"
glass = \"system\"
visualizer_fps = 60
visualizer_modes = [\"spectrum\", \"wave\"]

[rainbow]
speed = 0.5
saturation = 0.8

[[correction]]
device = \"alsa_output.usb-headphones\"
gains = [1.5, -2.0, 0.0]
",
        )
        .expect("a later version's keys");
        Settings {
            power: false,
            output_preset: "Rock".to_owned(),
            windows_parity: WindowsParity::Sound,
            device_configs: vec![
                DeviceConfig {
                    device_id: "alsa_output.usb-headphones".to_owned(),
                    preset: "Rock".to_owned(),
                    device_form_factor: "headphone".to_owned(),
                    extra: nested("preset_by_type = true\n[eq_trim]\nbass = -1.5\n"),
                    ..DeviceConfig::default()
                },
                DeviceConfig {
                    device_id: "alsa_output.pci-speakers".to_owned(),
                    preset: "General".to_owned(),
                    ..DeviceConfig::default()
                },
            ],
            device_volumes: vec![TargetVolume {
                target: "alsa_output.usb-headphones".to_owned(),
                channel_volumes: vec![0.5, 0.5],
                extra: nested("balance_curve = \"equal-power\"\n"),
                ..TargetVolume::default()
            }],
            calibration: Some(CalibrationRecord {
                noise_floor_db: -48.0,
                preset: "Calibrated".to_owned(),
                extra: nested("hum_hz = 50\nbands = [120.0, 4000.0]\n"),
                ..CalibrationRecord::default()
            }),
            extra: later,
            ..Settings::default()
        }
    }

    #[test]
    fn keys_of_a_later_version_survive_a_load_and_a_save_byte_for_byte() {
        // A10: going back a version and up again must not cost the later version's keys.
        let dir = tempfile::tempdir().expect("a scratch directory");
        let path = dir.path().join("settings.toml");
        with_later_keys().save_to(&path).expect("save");
        let first = std::fs::read(&path).expect("read back");

        let loaded = Settings::load_from(&path);
        assert!(!Settings::bad_path(&path).exists());
        assert_eq!(loaded, {
            let mut expected = with_later_keys();
            expected.sanitise();
            expected
        });
        loaded.save_to(&path).expect("save again");
        let second = std::fs::read(&path).expect("read back");
        assert_eq!(
            String::from_utf8_lossy(&first),
            String::from_utf8_lossy(&second)
        );
        assert_eq!(first, second);

        // And the nested keys are where the later version put them, not hoisted to the top.
        let text = String::from_utf8_lossy(&second);
        for line in [
            "preset_by_type = true",
            "[device_configs.eq_trim]",
            "balance_curve = \"equal-power\"",
            "hum_hz = 50",
        ] {
            assert!(text.contains(line), "missing {line:?} in:\n{text}");
        }
        let top: toml::Table = toml::from_str(&text).expect("parse");
        for key in [
            "preset_by_type",
            "eq_trim",
            "balance_curve",
            "hum_hz",
            "bands",
        ] {
            assert!(!top.contains_key(key), "{key} moved to the top of:\n{text}");
        }
    }

    #[test]
    fn keys_this_version_does_not_know_are_kept_wherever_a_hand_put_them() {
        // Written by hand, in no order a save would choose: the unknown keys come back with the
        // same values, whatever order the next save puts them in.
        let text = "\
future_first = \"kept\"
power = false
nested_future = { a = 1, b = [true, false] }
output_preset = \"Jazz\"
windows_parity = \"interface\"
future_last = 2.5

[future_table]
name = \"x\"

[[device_configs]]
device_id = \"alsa_output.usb-headphones\"
future_nested = \"x\"
preset = \"Rock\"

[[device_volumes]]
future_volume = { curve = \"db\" }
target = \"alsa_output.usb-headphones\"
channel_volumes = [0.5]

[calibration]
future_cal = 3
noise_floor_db = -48.0
";
        let dir = tempfile::tempdir().expect("a scratch directory");
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, text).expect("write");
        let loaded = Settings::load_from(&path);
        assert!(!Settings::bad_path(&path).exists());
        assert_eq!(loaded.output_preset, "Jazz");
        assert_eq!(loaded.windows_parity, WindowsParity::Interface);
        let keys: Vec<&str> = loaded.extra.keys().map(String::as_str).collect();
        assert_eq!(
            keys.iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>(),
            [
                "future_first",
                "future_last",
                "future_table",
                "nested_future"
            ]
            .into_iter()
            .collect()
        );

        loaded.save_to(&path).expect("save");
        let saved: toml::Table =
            toml::from_str(&std::fs::read_to_string(&path).expect("read back")).expect("parse");
        let original: toml::Table = toml::from_str(text).expect("parse");
        for key in keys {
            assert_eq!(saved.get(key), original.get(key), "{key}");
        }
        let entry = |table: &toml::Table, array: &str, key: &str| {
            table[array].as_array().expect("an array of tables")[0]
                .get(key)
                .cloned()
        };
        assert_eq!(
            entry(&saved, "device_configs", "future_nested"),
            Some(toml::Value::from("x"))
        );
        assert_eq!(
            entry(&saved, "device_volumes", "future_volume"),
            entry(&original, "device_volumes", "future_volume")
        );
        assert_eq!(
            saved["calibration"].get("future_cal"),
            Some(&toml::Value::from(3))
        );
        assert_eq!(Settings::load_from(&path), loaded);
    }

    #[test]
    fn a_device_the_app_updates_keeps_the_keys_a_later_version_wrote_into_its_entry() {
        let mut settings: Settings = toml::from_str(
            "[[device_configs]]\ndevice_id = \"hdmi\"\npreset = \"Rock\"\nfuture = 1\n\n\
             [[device_volumes]]\ntarget = \"hdmi\"\nchannel_volumes = [0.5]\nfuture = 2\n",
        )
        .expect("parse");
        settings.sanitise();

        settings.remember_device_preset("hdmi", "TV", "Jazz", "tv", DeviceDirection::Output);
        assert_eq!(settings.device_configs[0].preset, "Jazz");
        assert_eq!(
            settings.device_configs[0].extra.get("future"),
            Some(&toml::Value::from(1))
        );

        let reported = |level: f32| TargetVolume {
            target: "hdmi".to_owned(),
            channel_volumes: vec![level],
            ..TargetVolume::default()
        };
        assert!(
            !settings.remember_target_volume(reported(0.5)),
            "the engine reports no later keys, and their absence is no change"
        );
        assert!(settings.remember_target_volume(reported(0.25)));
        assert_eq!(settings.device_volumes[0].channel_volumes, [0.25]);
        assert_eq!(
            settings.device_volumes[0].extra.get("future"),
            Some(&toml::Value::from(2)),
            "a new level for the device is not a reason to forget the rest of its entry"
        );
    }

    #[test]
    fn a_calibration_record_dropped_as_corrupt_goes_with_the_later_keys_inside_it() {
        // The keys inside `[calibration]` describe the same run as the rest of the record.
        let mut corrupt: Settings =
            toml::from_str("[calibration]\nnoise_floor_db = nan\nfuture_cal = 3\n").expect("parse");
        assert_eq!(corrupt.calibration.as_ref().map(|r| r.extra.len()), Some(1));
        corrupt.sanitise();
        assert_eq!(corrupt.calibration, None);
        assert!(
            corrupt.extra.is_empty(),
            "not hoisted to the top of the file"
        );
    }
}
