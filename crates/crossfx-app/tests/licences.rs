//! What the README says of the licences has to be what the tree ships: a packager reads its
//! Licence section to tell which files carry terms other than CrossFX's own, and its lists of what
//! the archive and the packages install beside `LICENSE`.
//!
//! Two of its claims had gone stale. The section counted two parts of the binary under licences
//! of their own, where it carries three, and both lists left out the third, Manrope, the interface
//! face. And it gave "the bundled artwork" to the original's repository as a whole after CrossFX's
//! own icon had taken the place of FxSound LLC's bars in it. So the notices are read from the
//! script that installs them, the artwork from the tree, and the section has to name each one.

use std::fs;
use std::path::{Path, PathBuf};

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

/// The README's Licence section: its heading and everything up to the next heading of its level.
fn licence_section(readme: &str) -> &str {
    let start = readme
        .find("\n## Licence\n")
        .expect("README.md has a Licence section")
        + 1;
    let section = &readme[start..];
    let end = section.find("\n## ").map_or(section.len(), |end| end + 1);
    &section[..end]
}

/// The notices of the parts of the binary under licences of their own, by the names
/// `packaging/build-tarball.sh` installs them under in `share/doc/crossfx`, beside `LICENSE`
/// (`COPYING.rnnoise`, `OFL.noto`, …). `tests/packaging.rs` holds every package to the same ones.
fn notices() -> Vec<String> {
    text("packaging/build-tarball.sh")
        .lines()
        .filter_map(|line| {
            let destination = line
                .strip_prefix("install -Dm644 ")?
                .split_whitespace()
                .nth(1)?;
            destination
                .trim_matches('"')
                .strip_prefix("${pkg}/share/doc/crossfx/")
                .map(str::to_owned)
        })
        .collect()
}

/// Every folder of `assets/`, where all the artwork the binary and the packages carry lives, that
/// holds an SVG, PNG or ICO file, as a path from the repository with `/` between its parts. Not
/// `packaging/`: a `makepkg` run there leaves the icons it installed in its `pkg/`.
fn artwork_folders() -> Vec<String> {
    let mut folders = Vec::new();
    let mut pending = vec![repo().join("assets")];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).expect("the directory is listed") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            let artwork = path.extension().is_some_and(|extension| {
                ["svg", "png", "ico"]
                    .iter()
                    .any(|kind| extension.eq_ignore_ascii_case(kind))
            });
            if !artwork {
                continue;
            }
            let folder = path
                .parent()
                .expect("a file has a folder")
                .strip_prefix(repo())
                .expect("under the repository")
                .components()
                .map(|part| part.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            if !folders.contains(&folder) {
                folders.push(folder);
            }
        }
    }
    folders.sort();
    folders
}

/// The Licence section names the notice of every part of the binary under a licence of its own,
/// and the README's lists of what the archive's `share/doc/crossfx` and the packages'
/// `/usr/share/licenses/crossfx` hold name each such part.
#[test]
fn the_readme_names_the_notice_of_every_part_of_the_binary_under_a_licence_of_its_own() {
    let notices = notices();
    for expected in ["COPYING.rnnoise", "OFL.noto", "OFL.manrope"] {
        assert!(
            notices.iter().any(|notice| notice == expected),
            "packaging/build-tarball.sh no longer installs {expected}: {notices:?}"
        );
    }
    let readme = text("README.md");
    let section = licence_section(&readme);
    let archive = readme
        .split_once("`share/doc/crossfx/` (")
        .and_then(|(_, rest)| rest.split_once(')'))
        .map(|(list, _)| list)
        .expect("README.md says what the archive's share/doc/crossfx holds");
    let packages = readme
        .lines()
        .find(|line| line.starts_with("| `/usr/share/licenses/crossfx/` |"))
        .expect("README.md says what /usr/share/licenses/crossfx holds");
    for notice in &notices {
        assert!(
            section.contains(&format!("`{notice}`")),
            "README.md's Licence section does not name {notice}:\n{section}"
        );
        // COPYING.rnnoise is RNNoise's notice, OFL.manrope Manrope's.
        let part = notice.rsplit('.').next().expect("a name");
        for (what, list) in [
            ("the archive's share/doc/crossfx", archive),
            ("/usr/share/licenses/crossfx", packages),
        ] {
            assert!(
                list.to_lowercase().contains(part),
                "README.md leaves the licence of {part} out of what {what} holds: {list}"
            );
        }
    }
}

/// The Licence section says whose the artwork in each folder of it is, by naming the folder: what
/// the original's repository gave and what CrossFX drew are told apart folder by folder, not
/// given to one of them as a whole.
#[test]
fn the_readme_names_every_folder_of_artwork_the_tree_ships() {
    let folders = artwork_folders();
    for expected in ["assets/icons/apps", "assets/images", "assets/windows"] {
        assert!(
            folders.iter().any(|folder| folder == expected),
            "no artwork in {expected}: {folders:?}"
        );
    }
    let readme = text("README.md");
    let section = licence_section(&readme);
    for folder in &folders {
        assert!(
            section.contains(&format!("`{folder}")),
            "README.md's Licence section does not say whose the artwork in {folder} is:\n{section}"
        );
    }
}
