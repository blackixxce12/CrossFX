//! Where the stores built from the standard locations look for the shipped presets, and when a
//! binary counts as running from a checkout (`source_tree_of`).
//!
//! A test of its own rather than one of the library's: it goes through the public API only, and
//! it runs as a binary that is not a development build of FxSound (`target/<profile>/deps/…`, or
//! deeper where cargo keeps its build directory apart). The old rule, "the directory three levels
//! above the binary is the checkout", took a directory in `target` for one all the same; the
//! stores must search nothing but the installation for such a binary, which is what an installed
//! `C:\Program Files\FxSound Port\fxsound.exe` is too.

use fxsound_core::test_support::ScratchDir;
use fxsound_preset::input::InputPreset;
use fxsound_preset::{PresetStore, install_data_dirs, source_tree_of};
use std::path::{Path, PathBuf};

/// A directory with what a checkout has, a `Cargo.toml` file and `assets/presets`, made where
/// any account could make them three levels above an installed binary.
fn fake_checkout(tag: &str) -> (ScratchDir, PathBuf) {
    let tmp = ScratchDir::new(tag);
    let root = tmp.join("root");
    std::fs::create_dir_all(root.join("assets").join("presets").join("Factsoft")).expect("mkdir");
    std::fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write");
    (tmp, root)
}

#[test]
fn a_binary_cargo_built_in_this_checkout_runs_from_it() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for profile in ["debug", "release"] {
        let exe = root.join("target").join(profile).join("fxsound");
        assert_eq!(source_tree_of(&exe), Some(root.clone()), "{profile}");
    }
}

#[test]
fn only_cargos_own_profile_directories_in_a_checkout_are_a_source_tree() {
    let (_tmp, root) = fake_checkout("source-tree-layouts");
    assert_eq!(
        source_tree_of(&root.join("target").join("release").join("fxsound")),
        Some(root.clone())
    );
    for exe in [
        // Three levels below the checkout's files, the depth that used to be enough: the
        // installers' folder two levels below a drive's root, and a .zip unpacked anywhere.
        root.join("Program Files")
            .join("FxSound Port")
            .join("fxsound.exe"),
        root.join("Apps").join("FxSound").join("fxsound.exe"),
        // A test harness, a cross build and an example are not the application either.
        root.join("target/debug/deps/fxsound_preset-0123456789abcdef"),
        root.join("target/x86_64-pc-windows-msvc/release/fxsound.exe"),
        root.join("target/release/examples/write_preset"),
        // A profile's name outside `target`, and `target` with something else below it.
        root.join("build/release/fxsound"),
        root.join("target/FxSound Port/fxsound.exe"),
    ] {
        assert_eq!(source_tree_of(&exe), None, "{}", exe.display());
    }
}

#[test]
fn cargos_layout_without_a_checkout_around_it_is_no_source_tree() {
    let tmp = ScratchDir::new("source-tree-bare");
    let root = tmp.join("root");
    let exe = root.join("target").join("release").join("fxsound");
    std::fs::create_dir_all(root.join("assets").join("presets")).expect("mkdir");
    assert_eq!(source_tree_of(&exe), None, "presets without a Cargo.toml");
    std::fs::create_dir(root.join("Cargo.toml")).expect("mkdir");
    assert_eq!(source_tree_of(&exe), None, "a folder named Cargo.toml");
    for exe in ["fxsound", "/fxsound", "/usr/bin/fxsound"] {
        assert_eq!(source_tree_of(Path::new(exe)), None, "{exe}");
    }
}

#[cfg(windows)]
#[test]
fn windows_matches_the_profile_and_target_names_without_regard_to_case() {
    let (_tmp, root) = fake_checkout("source-tree-case");
    assert_eq!(
        source_tree_of(&root.join(r"Target\Release\fxsound.exe")),
        Some(root.clone())
    );
}

/// Whether `dir` is one of the installation's preset directories, or inside one.
fn is_installed(dir: &Path) -> bool {
    install_data_dirs()
        .iter()
        .any(|data| dir.starts_with(data.join("presets")))
}

#[test]
fn a_binary_that_is_no_development_build_takes_shipped_presets_from_its_installation_only() {
    let exe = std::env::current_exe().expect("the test's own path");
    assert_eq!(source_tree_of(&exe), None, "{}", exe.display());
    let factory = PresetStore::with_default_dirs().factory_dirs().to_vec();
    assert_eq!(factory.len(), 2 * install_data_dirs().len(), "{factory:?}");
    for dir in &factory {
        assert!(is_installed(dir), "factory presets from {}", dir.display());
    }
    let voice = InputPreset::default_dirs();
    assert_eq!(voice.len(), install_data_dirs().len(), "{voice:?}");
    for dir in &voice {
        assert!(is_installed(dir), "voice presets from {}", dir.display());
    }
}
