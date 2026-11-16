//! The Linux files the packages ship, held against the names the code goes by
//! (`fxsound_core::identity`, the app's public constants), on every system: a desktop file, a
//! unit, an activation file, the metainfo, the manual page, the autostart example, the status
//! icons and the Hyprland example are read here as they lie in the tree, so a name that one of
//! them spells otherwise fails on this machine and not only on Linux CI.
//!
//! The tests that hold the same files against the Linux-only code (the self-test's unit and
//! activation-file checks, `dbus.rs`'s manual-page check, `tests/packaging.rs`) run on Linux only.

use std::fs;
use std::path::{Path, PathBuf};

use fxsound_core::identity::{APP_ID, BINARY, PRODUCT};

/// The repository this test binary was built from.
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root exists")
}

/// A file of the tree as text, its lines ending in LF whatever the checkout made of them.
fn text(relative: &str) -> String {
    let path = repo().join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} could not be read: {error}", path.display()))
        .replace("\r\n", "\n")
}

/// The value of `key=` in an INI-style file (a desktop entry, a unit, an activation file), the
/// first one; untranslated keys only.
fn value<'a>(file: &'a str, key: &str) -> Option<&'a str> {
    file.lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        (name.trim() == key).then(|| value.trim())
    })
}

/// [`value`], which the file has to have.
fn required<'a>(file: &'a str, key: &str, name: &str) -> &'a str {
    value(file, key).unwrap_or_else(|| panic!("{name} has no {key}="))
}

/// The application id as a regex matches it exactly: its dots escaped.
fn escaped_app_id() -> String {
    APP_ID.replace('.', r"\.")
}

#[test]
fn the_desktop_file_is_named_after_the_application_id_and_starts_crossfx() {
    let name = format!("packaging/{APP_ID}.desktop");
    let desktop = text(&name);
    assert!(desktop.starts_with("[Desktop Entry]\n"), "{name}");
    assert_eq!(required(&desktop, "Name", &name), PRODUCT);
    let exec = required(&desktop, "Exec", &name);
    assert_eq!(
        exec.split_whitespace().next(),
        Some(BINARY),
        "{name}: Exec={exec}"
    );
    assert_eq!(required(&desktop, "Icon", &name), APP_ID);
    assert_eq!(required(&desktop, "StartupWMClass", &name), APP_ID);
}

#[test]
fn the_user_unit_owns_the_bus_name_and_starts_and_stops_crossfx() {
    let name = "packaging/crossfx.service";
    let unit = text(name);
    assert_eq!(required(&unit, "BusName", name), APP_ID);
    assert_eq!(
        required(&unit, "ExecStart", name),
        format!("/usr/bin/{BINARY} --activated")
    );
    let stop = required(&unit, "ExecStop", name);
    assert!(
        stop.contains(&format!("/usr/bin/{BINARY} --quit")),
        "{name}: ExecStop={stop}"
    );
    assert_eq!(
        required(&unit, "Documentation", name),
        format!("man:{BINARY}(1)")
    );
}

#[test]
fn the_activation_file_is_named_after_the_bus_name_and_hands_the_start_to_the_unit() {
    let name = format!("packaging/{APP_ID}.service");
    let activation = text(&name);
    assert!(
        activation
            .lines()
            .any(|line| line.trim() == "[D-BUS Service]"),
        "{name}"
    );
    assert_eq!(required(&activation, "Name", &name), APP_ID);
    assert_eq!(
        required(&activation, "Exec", &name),
        format!("/usr/bin/{BINARY} --activated")
    );
    assert_eq!(
        required(&activation, "SystemdService", &name),
        format!("{BINARY}.service")
    );
}

#[test]
fn the_metainfo_is_the_application_ids_and_provides_the_binary_and_the_bus_name() {
    let name = format!("packaging/{APP_ID}.metainfo.xml");
    let metainfo = text(&name);
    for wanted in [
        format!("<id>{APP_ID}</id>"),
        format!(r#"<launchable type="desktop-id">{APP_ID}.desktop</launchable>"#),
        format!("<binary>{BINARY}</binary>"),
        format!(r#"<dbus type="user">{APP_ID}</dbus>"#),
    ] {
        assert!(metainfo.contains(&wanted), "{name} has no {wanted}");
    }
}

#[test]
fn the_manual_page_is_crossfx_1_of_this_version_and_names_the_bus() {
    let name = format!("packaging/{BINARY}.1");
    let page = text(&name);
    let heading = page
        .lines()
        .find(|line| line.starts_with(".TH "))
        .unwrap_or_else(|| panic!("{name} has no .TH line"));
    assert!(
        heading.starts_with(&format!(".TH {} 1 \"", BINARY.to_uppercase())),
        "{name}: {heading}"
    );
    assert!(
        heading.contains(&format!("\"{BINARY} {}\"", env!("CARGO_PKG_VERSION"))),
        "{name}: {heading}"
    );
    let object_path = format!("/{}", APP_ID.replace('.', "/"));
    for wanted in [APP_ID, object_path.as_str()] {
        assert!(page.contains(wanted), "{name} does not name {wanted}");
    }
}

#[test]
fn the_shipped_autostart_example_is_the_entry_the_application_writes() {
    let name = "packaging/crossfx-autostart.desktop";
    // Byte for byte, line endings included: the file is what "Launch on system startup" writes.
    let path = repo().join(name);
    let shipped = fs::read(&path)
        .unwrap_or_else(|error| panic!("{} could not be read: {error}", path.display()));
    assert_eq!(
        String::from_utf8_lossy(&shipped),
        fxsound_app::platform::AUTOSTART_ENTRY,
        "{name}"
    );
}

#[test]
fn every_status_icon_the_tray_names_is_in_the_tree() {
    for icon in [
        fxsound_app::tray::ICON_OFF,
        fxsound_app::tray::ICON_ON,
        fxsound_app::tray::ICON_PROCESSING,
    ] {
        let path = repo().join(format!("assets/icons/status/{icon}.svg"));
        assert!(path.is_file(), "{} is not in the tree", path.display());
    }
}

#[test]
fn the_hyprland_example_rules_the_window_by_its_class_and_binds_crossfx() {
    let name = "packaging/hyprland.conf.example";
    let example = text(name);
    let rule = format!("match:class = ^({})$", escaped_app_id());
    assert!(
        example.lines().any(|line| line.trim() == rule),
        "{name} has no `{rule}`"
    );
    let binds: Vec<&str> = example
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(|line| line.split_once("exec, ").map(|(_, command)| command.trim()))
        .collect();
    assert!(!binds.is_empty(), "{name} binds nothing");
    for command in binds {
        assert!(
            command.starts_with(&format!("{BINARY} --")),
            "{name}: exec, {command}"
        );
    }
}
