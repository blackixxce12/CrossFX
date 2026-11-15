//! The names CrossFX goes by, for every crate that needs one.
//!
//! The single source of every name of the program that more than one crate uses: the window and
//! the tray, the desktop files, D-Bus, PipeWire's nodes and the folders under the system's
//! directories all take it from here. None of them is an earlier version's name: CrossFX looks
//! for nothing an earlier version left.
//!
//! "FxSound" is not among them. It is the original FxSound by FxSound LLC, whose files, folders
//! and registry values CrossFX never touches — on Windows `%APPDATA%\FxSound` and
//! `%LOCALAPPDATA%\FxSound` are the original's, and Windows does not tell `fxsound` from
//! `FxSound`.

/// The product's name, as people read it: window title, tray, notifications, sound settings.
pub const PRODUCT: &str = "CrossFX";

/// Desktop-entry id, Wayland app_id / X11 class, AppStream id, D-Bus name, icon name,
/// PipeWire application.id, Windows AppUserModelID.
pub const APP_ID: &str = "io.github.blackixxce12.CrossFX";

/// The executable's stem: `crossfx`, `crossfx.exe`.
pub const BINARY: &str = "crossfx";

/// CrossFX's folder under each system directory: `crossfx` under the XDG configuration, data,
/// state, cache and runtime directories, and `CrossFX` under `%APPDATA%` and `%LOCALAPPDATA%`.
pub const DIR_NAME: &str = if cfg!(windows) { "CrossFX" } else { "crossfx" };

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether `name` is a D-Bus well-known name and a desktop-file id: dot-separated elements of
    /// `[A-Za-z0-9_]`, none empty and none starting with a digit, at least two dots, under 255
    /// bytes in all.
    fn is_bus_name_and_desktop_id(name: &str) -> bool {
        let elements: Vec<&str> = name.split('.').collect();
        name.len() < 255
            && elements.len() >= 3
            && elements.iter().all(|element| {
                !element.is_empty()
                    && !element.starts_with(|c: char| c.is_ascii_digit())
                    && element
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_')
            })
    }

    #[test]
    fn the_application_id_is_a_bus_name_a_desktop_id_and_an_app_user_model_id() {
        assert!(is_bus_name_and_desktop_id(APP_ID), "{APP_ID}");
        // An AppUserModelID: no spaces, and under 129 characters.
        assert!(!APP_ID.contains(char::is_whitespace), "{APP_ID}");
        assert!(APP_ID.chars().count() < 129, "{APP_ID}");
        // The checks themselves tell a good name from a bad one.
        for bad in [
            "CrossFX",
            "io.github",
            "io..CrossFX",
            "io.github.9CrossFX",
            "io.github.Cross-FX",
            "io.github.Cross FX",
        ] {
            assert!(!is_bus_name_and_desktop_id(bad), "{bad}");
        }
    }

    #[test]
    fn the_folder_is_crossfx_and_capitalised_on_windows() {
        if cfg!(windows) {
            assert_eq!(DIR_NAME, "CrossFX");
        } else {
            assert_eq!(DIR_NAME, "crossfx");
        }
        assert!(DIR_NAME.eq_ignore_ascii_case(BINARY));
        assert!(DIR_NAME.eq_ignore_ascii_case(PRODUCT));
    }

    #[test]
    fn no_name_of_crossfx_is_the_original_fxsound_in_any_case() {
        for name in [PRODUCT, APP_ID, BINARY, DIR_NAME] {
            assert!(!name.to_lowercase().contains("fxsound"), "{name}");
        }
        assert!(APP_ID.ends_with(PRODUCT));
    }
}
