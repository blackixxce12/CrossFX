//! The Windows installers and their smoke test, held against the names the code goes by
//! (`crossfx_core::identity`), on every system: `packaging/windows/crossfx.iss`, `crossfx.wxs` and
//! `smoke-test.ps1` are read here as they lie in the tree.
//!
//! Two things are held. The names CrossFX installs, registers and deletes are CrossFX's own: the
//! product, the publisher, the folders, the program, the "Run at startup" value, the
//! AppUserModelID of its notifications, and the setup program's AppId and the package's
//! UpgradeCode, each with the mirror the other installer keeps of it. And none of the three files
//! names a registry value, key or folder of the original FxSound by FxSound LLC — its Run value,
//! its keys under `HKCU\Software`, its AppUserModelID, its folders in the application data — not
//! to write it, not to read it, not to assert it untouched, not in a comment: they are the
//! original's whatever their case, since Windows does not tell one spelling from another.

use std::fs;
use std::path::{Path, PathBuf};

use crossfx_core::identity::{APP_ID, BINARY, DIR_NAME, PRODUCT};

/// The setup program's AppId, fixed for good.
const SETUP_APP_ID: &str = "50CFC36F-E858-4CEB-832E-3ED2BC72D008";
/// The package's UpgradeCode, fixed for good.
const MSI_UPGRADE_CODE: &str = "02B08DA0-2631-41C0-8125-7D6D5CFDF025";

/// The repository this test binary was built from.
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root exists")
}

/// A file of `packaging/windows` as text, its lines ending in LF whatever the checkout made of
/// them.
fn text(name: &str) -> String {
    let path = repo().join("packaging/windows").join(name);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} could not be read: {error}", path.display()))
        .replace("\r\n", "\n")
}

/// The value of `#define <name> "<value>"` in an Inno Setup script.
fn define<'a>(iss: &'a str, name: &str) -> &'a str {
    iss.lines()
        .find_map(|line| {
            let rest = line.trim().strip_prefix("#define")?.trim_start();
            let rest = rest.strip_prefix(name)?;
            if !rest.starts_with(char::is_whitespace) {
                return None;
            }
            rest.trim().strip_prefix('"')?.strip_suffix('"')
        })
        .unwrap_or_else(|| panic!("crossfx.iss has no #define {name} \"…\""))
}

/// Every opening tag of `element` in `xml`, attributes and all.
fn tags<'a>(xml: &'a str, element: &str) -> Vec<&'a str> {
    let open = format!("<{element}");
    xml.match_indices(&open)
        .filter(|(at, _)| {
            xml[at + open.len()..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
        })
        .map(|(at, _)| {
            let tag = &xml[at..];
            &tag[..tag.find('>').expect("the tag ends")]
        })
        .collect()
}

/// The value of `attribute` in `tag`, a whole attribute name.
fn attribute<'a>(tag: &'a str, attribute: &str) -> Option<&'a str> {
    let key = format!("{attribute}=\"");
    let at = tag
        .match_indices(&key)
        .find(|(at, _)| tag[..*at].ends_with(char::is_whitespace))
        .map(|(at, _)| at + key.len())?;
    let value = &tag[at..];
    Some(&value[..value.find('"')?])
}

/// The one tag of `element` whose `id_attribute` is `id`.
fn tag_with<'a>(xml: &'a str, element: &str, id_attribute: &str, id: &str) -> &'a str {
    let found: Vec<&str> = tags(xml, element)
        .into_iter()
        .filter(|tag| attribute(tag, id_attribute) == Some(id))
        .collect();
    let [tag] = found.as_slice() else {
        panic!(
            "crossfx.wxs has {} <{element} {id_attribute}=\"{id}\">",
            found.len()
        );
    };
    tag
}

/// The value a PowerShell script assigns to `$<name>` with a single-quoted string.
fn assigned<'a>(ps1: &'a str, name: &str) -> &'a str {
    ps1.lines()
        .find_map(|line| {
            let rest = line.trim().strip_prefix('$')?.strip_prefix(name)?;
            let rest = rest.trim_start().strip_prefix('=')?.trim();
            rest.strip_prefix('\'')?.strip_suffix('\'')
        })
        .unwrap_or_else(|| panic!("smoke-test.ps1 assigns no ${name} = '…'"))
}

#[test]
fn the_setup_program_installs_crossfx_under_crossfx_names() {
    let iss = text("crossfx.iss");
    assert_eq!(define(&iss, "AppName"), PRODUCT);
    // The folder in Program Files and in %LOCALAPPDATA%: CrossFX's on Windows, where the code
    // calls it `identity::DIR_NAME`.
    assert_eq!(define(&iss, "AppDirName"), "CrossFX");
    assert!(define(&iss, "AppDirName").eq_ignore_ascii_case(DIR_NAME));
    assert_eq!(
        define(&iss, "AppPublisher"),
        format!("{PRODUCT} contributors")
    );
    assert_eq!(
        define(&iss, "AppURL"),
        "https://github.com/blackixxce12/CrossFX"
    );
    assert_eq!(define(&iss, "AppExeName"), format!("{BINARY}.exe"));
    // What `win::autostart` and `win::toast` write: the Run value and the AppUserModelID.
    assert_eq!(define(&iss, "AutostartValue"), PRODUCT);
    assert_eq!(define(&iss, "AppUserModelId"), APP_ID);
    assert!(
        iss.lines()
            .any(|line| line.trim() == format!("AppId={{{{{SETUP_APP_ID}}}")),
        "crossfx.iss has no AppId={{{{{SETUP_APP_ID}}}"
    );
    // The mirror of the package's UpgradeCode, by which the setup program tells that the .msi is
    // installed.
    assert!(
        iss.contains(&format!("'{{{MSI_UPGRADE_CODE}}}'")),
        "crossfx.iss does not name the .msi's UpgradeCode {MSI_UPGRADE_CODE}"
    );
    // The local folder the uninstaller cleans up is CrossFX's own, and no other.
    let local: Vec<&str> = iss
        .match_indices("{localappdata}\\")
        .map(|(at, _)| &iss[at..])
        .collect();
    assert!(!local.is_empty(), "crossfx.iss names no local folder");
    for path in local {
        assert!(
            path.starts_with("{localappdata}\\{#AppDirName}"),
            "crossfx.iss names a local folder that is not CrossFX's: {}",
            path.lines().next().unwrap_or_default()
        );
    }
}

#[test]
fn the_package_installs_crossfx_under_crossfx_names() {
    let wxs = text("crossfx.wxs");
    let package = tags(&wxs, "Package");
    let [package] = package.as_slice() else {
        panic!("crossfx.wxs has {} <Package> tags", package.len());
    };
    assert_eq!(attribute(package, "Name"), Some(PRODUCT));
    assert_eq!(
        attribute(package, "Manufacturer"),
        Some(format!("{PRODUCT} contributors").as_str())
    );
    assert_eq!(attribute(package, "UpgradeCode"), Some(MSI_UPGRADE_CODE));
    for folder in ["INSTALLFOLDER", "NotificationIconFolder"] {
        assert_eq!(
            attribute(tag_with(&wxs, "Directory", "Id", folder), "Name"),
            Some("CrossFX"),
            "{folder}"
        );
    }
    let program = tags(&wxs, "File")
        .into_iter()
        .filter_map(|tag| attribute(tag, "Source"))
        .find(|source| source.ends_with(&format!("\\{BINARY}.exe")));
    assert!(program.is_some(), "crossfx.wxs installs no {BINARY}.exe");
    for wanted in [
        // Where the package keeps its own per-user state.
        format!(r"Software\{PRODUCT}\Installer"),
        // The registration `win::toast` writes, removed by the uninstall.
        format!(r"Software\Classes\AppUserModelId\{APP_ID}"),
        // The Run value `win::autostart` writes, deleted by name.
        format!("/v &quot;{PRODUCT}&quot; /f"),
        // The mirror of the setup program's AppId, by which the package tells that it is
        // installed.
        format!(r"Uninstall\{{{SETUP_APP_ID}}}_is1"),
    ] {
        assert!(wxs.contains(&wanted), "crossfx.wxs has no {wanted}");
    }
}

#[test]
fn the_smoke_test_looks_for_crossfx_and_installs_only_on_a_throw_away_runner() {
    let ps1 = text("smoke-test.ps1");
    assert_eq!(assigned(&ps1, "product"), PRODUCT);
    assert_eq!(
        assigned(&ps1, "publisher"),
        format!("{PRODUCT} contributors")
    );
    assert_eq!(assigned(&ps1, "appUserModelId"), APP_ID);
    assert!(ps1.contains(&format!("{BINARY}.exe")));
    assert!(
        ps1.contains("$env:GITHUB_ACTIONS -eq 'true'"),
        "smoke-test.ps1 does not keep -InstallOnThisMachine to GitHub Actions"
    );
    assert!(
        ps1.contains(
            "this mode installs and uninstalls CrossFX; it runs only on a throw-away GitHub \
             Actions runner"
        ),
        "smoke-test.ps1 does not say why -InstallOnThisMachine refuses"
    );
}

#[test]
fn no_installer_names_a_value_key_or_folder_of_the_original_fxsound() {
    for name in ["crossfx.iss", "crossfx.wxs", "smoke-test.ps1"] {
        // Windows does not tell `fxsound` from `FxSound`, so neither does this.
        let file = text(name).to_lowercase();
        let mut wrong = Vec::new();
        for quoted in ["'fxsound'", "\"fxsound\"", "&quot;fxsound&quot;"] {
            if file.contains(quoted) {
                wrong.push(format!("the quoted name {quoted}"));
            }
        }
        for key in [r"software\fxsound", r"appusermodelid\fxsound"] {
            if file.contains(key) {
                wrong.push(format!("the key {key}"));
            }
        }
        for folder in [
            "{localappdata}",
            "{userappdata}",
            "[localappdatafolder]",
            "[appdatafolder]",
            "%localappdata%",
            "%appdata%",
            "$env:localappdata",
            "$env:appdata",
        ] {
            for joined in [format!(r"{folder}\fxsound"), format!("{folder}fxsound")] {
                if file.contains(&joined) {
                    wrong.push(format!("the folder {joined}"));
                }
            }
        }
        assert!(
            wrong.is_empty(),
            "{name} names the original FxSound's {}",
            wrong.join(", ")
        );
    }
}
