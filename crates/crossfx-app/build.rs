//! The Windows resources of `crossfx.exe`: its icon, its version information and its application
//! manifest. Every other target builds exactly as it did before this script existed.
//!
//! * The icon is what Explorer, Start and the installers' shortcuts show for the executable;
//!   without it they show Windows' blank default. `assets/windows/crossfx.ico`, made from
//!   `assets/images/fxsound_large.png` by `packaging/windows/make-icon.ps1`.
//! * The version information is the Details tab of the file's properties, and its
//!   `FileDescription` is the name Task Manager lists the process under. It says "CrossFX" and
//!   CrossFX's contributors, never FxSound LLC's product name, because the original FxSound may
//!   be installed and running beside it.
//! * The manifest (`assets/windows/crossfx.exe.manifest`) declares per-monitor DPI awareness,
//!   Common Controls 6, Windows 10 and 11, and `asInvoker`; its comments say why each.
//!
//! A build script runs on the build machine, so what decides is the target being built
//! (`CARGO_CFG_TARGET_OS`), not `cfg(windows)`: a Linux build checked on a Windows host must get
//! nothing. `winresource` is a build-dependency of Windows hosts only (`Cargo.toml`), so a Linux
//! host never compiles it; a Windows binary cross-compiled from Linux, a build nobody makes, goes
//! without its resources and the build says so.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    resources::embed();
}

#[cfg(windows)]
mod resources {
    use std::path::{Path, PathBuf};

    /// The one binary that carries the resources. The library is linked into the test harnesses
    /// as well, and a test runner has no use for an icon or for a manifest that changes how it
    /// is run.
    ///
    /// It is `crossfx_core::identity::BINARY`, which a build script cannot name: a test of the
    /// library holds the two equal, and `PRODUCT` and `COMPANY` with them.
    const BINARY: &str = "crossfx";

    /// The publisher, as the installers name it too (`packaging/windows/`).
    const COMPANY: &str = "CrossFX contributors";
    /// The product, `crossfx_core::identity::PRODUCT`. Never "FxSound": that is FxSound LLC's
    /// product, which may be installed alongside.
    const PRODUCT: &str = "CrossFX";
    /// The notice the AGPL asks a program to carry, cut down to one line.
    const COPYRIGHT: &str = "Copyright \u{a9} CrossFX contributors, portions \u{a9} FxSound LLC. \
        Free software under the GNU Affero General Public License, version 3 or later; it comes \
        with ABSOLUTELY NO WARRANTY.";
    /// en-US, the language the strings above are in.
    const LANGUAGE: u16 = 0x0409;

    pub fn embed() {
        let assets = assets_dir();
        let icon = assets.join("crossfx.ico");
        let manifest = assets.join("crossfx.exe.manifest");
        for input in [&icon, &manifest] {
            println!("cargo::rerun-if-changed={}", input.display());
        }

        let mut resource = winresource::WindowsResource::new();
        // FileVersion, ProductVersion and the numeric versions are CARGO_PKG_VERSION already,
        // which is [workspace.package]'s version; the numeric ones drop a pre-release suffix,
        // since VERSIONINFO has room for four numbers only.
        resource
            .set_language(LANGUAGE)
            .set("CompanyName", COMPANY)
            .set("ProductName", PRODUCT)
            .set("FileDescription", PRODUCT)
            .set("InternalName", BINARY)
            .set("OriginalFilename", &format!("{BINARY}.exe"))
            .set("LegalCopyright", COPYRIGHT)
            .set_icon(&utf8(&icon))
            .set_manifest_file(&utf8(&manifest));
        // A binary without its manifest still starts, but blurry and unthemed, so it is not let
        // through as a warning: the resource compiler is part of the Windows SDK every MSVC build
        // already links with.
        if let Err(err) = resource.compile_for(BINARY) {
            panic!("could not compile the Windows resources of {BINARY}.exe: {err}");
        }
    }

    /// `assets/windows` of the repository this crate is in.
    fn assets_dir() -> PathBuf {
        let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR")
            .map(PathBuf::from)
            .expect("Cargo sets CARGO_MANIFEST_DIR for a build script");
        manifest_dir
            .ancestors()
            .nth(2)
            .map(|root| root.join("assets").join("windows"))
            .expect("crates/crossfx-app is two levels below the repository root")
    }

    /// The path as the resource script spells it. winresource writes the script as UTF-8
    /// (`#pragma code_page(65001)`), so a path through a profile folder named in Cyrillic, as on
    /// a Russian Windows, reaches rc.exe intact; only a path that is not Unicode at all (an
    /// unpaired surrogate, which NTFS allows and nobody uses) is refused.
    fn utf8(path: &Path) -> String {
        match path.to_str() {
            Some(path) => path.to_owned(),
            None => panic!("{} is not valid Unicode", path.display()),
        }
    }
}

#[cfg(not(windows))]
mod resources {
    pub fn embed() {
        println!(
            "cargo::warning=crossfx.exe is built without its icon, version information and \
             manifest: they are compiled by winresource, which is only built on a Windows host"
        );
    }
}
