//! What the README says of the licences has to be what the tree ships: a packager reads its
//! Licence section to tell which files carry terms other than CrossFX's own, and its lists of what
//! the archive and the packages install beside `LICENSE`. A `.deb`'s packager reads
//! `debian/copyright` instead, which has to give the artwork the holders the section gives it.
//!
//! The section has gone stale more than once. It counted two parts of the binary under licences
//! of their own where the tree has three, and both lists left out the third, Manrope, the
//! interface face; and it gave its count as all the binary carries under other licences, where
//! the crates the binary is built from carry their own, egui's faces among them. It gave "the
//! bundled artwork" to the original's repository as a whole after CrossFX's own icon had taken
//! the place of FxSound LLC's bars in it, and `assets/images` holds the original's images and the
//! ones CrossFX drew beside each other, so that a folder's name cannot say whose each one is. So
//! the notices are read from the script that installs them, and the section has to count them
//! and name each; egui's faces are named at the version of their crate `Cargo.lock` resolves; and
//! the artwork is read from the tree, folder by folder, and file by file in `assets/images`
//! against the list of the original's files below.

use std::fs;
use std::path::{Path, PathBuf};

/// The files of `assets/images` that are the original's: FxSound LLC's interface artwork, taken
/// across from `fxsound/Images` in its repository (`docs/spec/13-assets.md`). Every other image in
/// the folder is one CrossFX drew to match, which the Licence section names.
const ORIGINAL_IMAGES: [&str; 56] = [
    "Slider_Thumb.svg",
    "Slider_Thumb_blue.svg",
    "Slider_Thumb_bw.svg",
    "arrow_down.svg",
    "arrow_down_black.svg",
    "arrow_down_blue.svg",
    "arrow_down_white.svg",
    "arrow_next.svg",
    "arrow_next_blue.svg",
    "arrow_next_bw.svg",
    "arrow_prev.svg",
    "arrow_prev_blue.svg",
    "arrow_prev_bw.svg",
    "arrow_up.svg",
    "arrow_up_black.svg",
    "arrow_up_blue.svg",
    "arrow_up_white.svg",
    "donate.svg",
    "donate_blue.svg",
    "donate_hover.svg",
    "donate_hover_blue.svg",
    "dropdown_arrow_bw.svg",
    "dropdown_arrow_hover.svg",
    "dropdown_arrow_hover_blue.svg",
    "flip.svg",
    "flip_black.svg",
    "flip_blue.svg",
    "flip_white.svg",
    "maximize.svg",
    "maximize_black.svg",
    "maximize_hover.svg",
    "maximize_hover_blue.svg",
    "menu.svg",
    "menu_black.svg",
    "menu_hover.svg",
    "menu_hover_blue.svg",
    "min_window.svg",
    "min_window_black.svg",
    "min_window_hover.svg",
    "min_window_hover_blue.svg",
    "minimize.svg",
    "minimize_black.svg",
    "minimize_hover.svg",
    "minimize_hover_blue.svg",
    "power_off.svg",
    "power_off_black.svg",
    "power_on.svg",
    "power_on_blue.svg",
    "question.svg",
    "remove.svg",
    "restore_defaults.svg",
    "restore_defaults_black.svg",
    "restore_defaults_blue.svg",
    "restore_defaults_white.svg",
    "settings.svg",
    "speaker.svg",
];

/// The notices of the faces of egui's `epaint_default_fonts` that eframe's `default_fonts` feature
/// compiles into the binary (Ubuntu Light, Hack, egui's icons), by the names every package installs
/// them under, and the files of `assets/fonts` they come from. They are not parts of the tree, so
/// the Licence section counts them apart from the three it has.
const EGUI_FACE_NOTICES: [&str; 3] = ["UFL.ubuntu", "MIT.hack", "MIT.emoji-icons"];

/// The generated notices of the Rust crates compiled into the binary, at the root of the tree and
/// installed under the same name by every package (`tests/third_party_notices.rs` generates it).
const THIRD_PARTY: &str = "THIRD-PARTY-NOTICES.txt";

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

/// `text` with every run of whitespace, line breaks included, made one space: the README is
/// wrapped by hand, and a name may be broken across two lines anywhere.
fn flowing(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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

/// The notices of the parts of the tree compiled into the binary under licences of their own, by
/// the names `packaging/build-tarball.sh` installs them under in `share/doc/crossfx`, beside
/// `LICENSE` (`COPYING.rnnoise`, `OFL.noto`, …). `tests/packaging.rs` holds every Linux package to
/// the same ones, `tests/windows_installers.rs` the Windows packages.
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

/// Every SVG, PNG or ICO file under `assets/`, where all the artwork the binary and the packages
/// carry lives, as a path from the repository with `/` between its parts, in order. Not
/// `packaging/`: a `makepkg` run there leaves the icons it installed in its `pkg/`.
fn artwork() -> Vec<String> {
    let root = repo();
    let mut files = Vec::new();
    let mut pending = vec![root.join("assets")];
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
            if artwork {
                files.push(
                    path.strip_prefix(&root)
                        .expect("under the repository")
                        .components()
                        .map(|part| part.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/"),
                );
            }
        }
    }
    files.sort();
    files
}

/// The versions of `package` that `Cargo.lock` resolves: none when nothing depends on it.
fn locked_versions(package: &str) -> Vec<String> {
    let lock = text("Cargo.lock");
    let name = format!("name = \"{package}\"");
    let mut lines = lock.lines();
    let mut versions = Vec::new();
    while let Some(line) = lines.next() {
        if line == name {
            let version = lines
                .next()
                .and_then(|line| line.strip_prefix("version = \"")?.strip_suffix('"'))
                .unwrap_or_else(|| panic!("Cargo.lock gives {package} no version"));
            versions.push(version.to_owned());
        }
    }
    versions
}

/// A `Files` paragraph of `debian/copyright`: its patterns, and the holders its `Copyright` field
/// names.
struct Stanza {
    patterns: Vec<String>,
    holders: String,
}

/// The `Files` paragraphs of a machine-readable `debian/copyright`, in their order. Paragraphs
/// are parted by blank lines, and a line that starts with a space or a tab carries on the field
/// above it.
fn stanzas(copyright: &str) -> Vec<Stanza> {
    let mut stanzas = Vec::new();
    let mut fields: Vec<(String, String)> = Vec::new();
    for line in copyright.lines().chain([""]) {
        if line.trim().is_empty() {
            let field = |wanted: &str| {
                fields
                    .iter()
                    .find(|(name, _)| name == wanted)
                    .map(|(_, value)| value.clone())
            };
            if let Some(files) = field("Files") {
                stanzas.push(Stanza {
                    patterns: files.split_whitespace().map(str::to_owned).collect(),
                    holders: field("Copyright").unwrap_or_default(),
                });
            }
            fields.clear();
        } else if line.starts_with([' ', '\t']) {
            if let Some((_, value)) = fields.last_mut() {
                value.push('\n');
                value.push_str(line.trim());
            }
        } else if let Some((name, value)) = line.split_once(':') {
            fields.push((name.to_owned(), value.trim().to_owned()));
        }
    }
    stanzas
}

/// Whether the `debian/copyright` pattern matches `path`: `*` is any run of characters, `/`
/// included, and `?` any one.
fn covers(pattern: &[u8], path: &[u8]) -> bool {
    match (pattern.split_first(), path.split_first()) {
        (None, None) => true,
        (Some((b'*', rest)), _) => {
            covers(rest, path) || (!path.is_empty() && covers(pattern, &path[1..]))
        }
        (Some((b'?', rest)), Some((_, tail))) => covers(rest, tail),
        (Some((expected, rest)), Some((found, tail))) => expected == found && covers(rest, tail),
        _ => false,
    }
}

/// Whom `debian/copyright` gives `path` to: the `Copyright` of the last `Files` paragraph that
/// matches it, which is how the format settles an overlap.
fn holders<'a>(stanzas: &'a [Stanza], path: &str) -> &'a str {
    stanzas
        .iter()
        .rev()
        .find(|stanza| {
            stanza
                .patterns
                .iter()
                .any(|pattern| covers(pattern.as_bytes(), path.as_bytes()))
        })
        .map(|stanza| stanza.holders.as_str())
        .unwrap_or_else(|| panic!("no Files paragraph of debian/copyright covers {path}"))
}

/// The Licence section counts the parts of the tree compiled into the binary under licences of
/// their own and names the notice of each, and the README's lists of what the archive's
/// `share/doc/crossfx` and the packages' `/usr/share/licenses/crossfx` hold name each such part.
#[test]
fn the_readme_counts_and_names_the_notice_of_every_part_of_the_tree_under_a_licence_of_its_own() {
    let all = notices();
    for expected in EGUI_FACE_NOTICES {
        assert!(
            all.iter().any(|notice| notice == expected),
            "packaging/build-tarball.sh no longer installs {expected}: {all:?}"
        );
    }
    // The faces of egui and the generated notices are counted and named by the tests below; these
    // are the tree's own.
    let notices: Vec<String> = all
        .into_iter()
        .filter(|notice| !EGUI_FACE_NOTICES.contains(&notice.as_str()) && notice != THIRD_PARTY)
        .collect();
    for expected in ["COPYING.rnnoise", "OFL.noto", "OFL.manrope"] {
        assert!(
            notices.iter().any(|notice| notice == expected),
            "packaging/build-tarball.sh no longer installs {expected}: {notices:?}"
        );
    }
    let readme = text("README.md");
    let section = licence_section(&readme);
    let count = [
        "No", "One", "Two", "Three", "Four", "Five", "Six", "Seven", "Eight", "Nine",
    ]
    .get(notices.len())
    .unwrap_or_else(|| panic!("{} notices, and this test counts to nine", notices.len()));
    assert!(
        flowing(section).contains(&format!(
            "{count} parts of the tree are under other licences"
        )),
        "README.md's Licence section does not count the {} parts of the tree under other \
         licences, one for each notice in {notices:?}:\n{section}",
        notices.len()
    );
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

/// The binary also carries faces that are not in the tree: those of egui's
/// `epaint_default_fonts`, which eframe's `default_fonts` feature (the workspace's `Cargo.toml`)
/// compiles in and the window keeps behind its own (`crossfx_ui::typeface::definitions` starts
/// from egui's `FontDefinitions::default()`). The section names the crate at the version
/// `Cargo.lock` resolves, so that an egui which brings other faces has the section read again,
/// and the three faces 0.37.0 compiles in, each with its licence. With no such crate, it names
/// none.
#[test]
fn the_readme_names_the_faces_egui_compiles_into_the_binary() {
    let readme = text("README.md");
    let section = flowing(licence_section(&readme));
    let versions = locked_versions("epaint_default_fonts");
    if versions.is_empty() {
        assert!(
            !section.contains("epaint_default_fonts"),
            "Cargo.lock has no epaint_default_fonts, and README.md's Licence section still names \
             its faces:\n{section}"
        );
        return;
    }
    for version in &versions {
        assert!(
            section.contains(&format!("`epaint_default_fonts` {version}")),
            "README.md's Licence section does not name epaint_default_fonts {version}, the one \
             Cargo.lock resolves; read which faces it compiles in, and say so:\n{section}"
        );
    }
    for named in [
        "Ubuntu Light",
        "Ubuntu Font Licence 1.0",
        "Hack",
        "Bitstream Vera",
        "emoji-icon-font",
    ] {
        assert!(
            section.contains(named),
            "README.md's Licence section does not name {named}, of the faces egui compiles into \
             the binary:\n{section}"
        );
    }
    // Their licences make shipping the notice a condition of shipping the binary (the Ubuntu Font
    // Licence's first condition, the MIT and Bitstream Vera licences' notice clauses), so the
    // section names each notice and the tree holds each; the packages install them
    // (`tests/packaging.rs`, `tests/windows_installers.rs`).
    for (name, file) in EGUI_FACE_NOTICES.iter().zip([
        "assets/fonts/UFL-Ubuntu.txt",
        "assets/fonts/MIT-Hack.txt",
        "assets/fonts/MIT-emoji-icon-font.txt",
    ]) {
        assert!(
            section.contains(&format!("`{name}`")) && section.contains(&format!("`{file}`")),
            "README.md's Licence section does not name {name} and {file}, the notice of a face \
             egui compiles into the binary:\n{section}"
        );
        assert!(
            !text(file).trim().is_empty(),
            "{file}, the notice of a face egui compiles into the binary, is empty"
        );
    }
}

/// `debian/copyright` counts the parts of the tree that are the original's work, and says which,
/// in the comment of its `Files: *` paragraph: the DSP, the presets, the artwork and the
/// translation tables (the original's own JUCE `LocalisedStrings` files, which CrossFX carried
/// across unchanged, CrossFX's own strings being in `assets/translations/port/`). The count had
/// stayed at three after the tables were added to the tree.
#[test]
fn debian_copyright_counts_every_part_of_the_original_it_carries_across() {
    let copyright = text("packaging/debian/copyright");
    let start = copyright
        .find("\nFiles: *\n")
        .expect("debian/copyright has a Files: * paragraph");
    let paragraph = &copyright[start..];
    let paragraph = &paragraph[..paragraph.find("\n\n").expect("the paragraph ends")];
    let bullets = paragraph.matches("\n   * ").count();
    let words = [
        "No", "One", "Two", "Three", "Four", "Five", "Six", "Seven", "Eight", "Nine",
    ];
    let count = words[bullets].to_lowercase();
    let flat = flowing(paragraph);
    assert!(
        flat.contains(&format!("and {count} parts of it are the original's work")),
        "debian/copyright's Files: * comment lists {bullets} parts of the original's work but \
         counts another number:\n{paragraph}"
    );
    assert!(
        flat.contains(&format!("All {count} inherit the original FxSound's terms")),
        "debian/copyright's Files: * comment closes on another number than the {bullets} parts it \
         lists:\n{paragraph}"
    );
    for part in [
        "crates/crossfx-dsp",
        "assets/presets/Factsoft",
        "assets/images",
        "assets/translations/*.txt",
        "assets/translations/port/",
    ] {
        assert!(
            flat.contains(part),
            "debian/copyright's Files: * comment does not name {part}:\n{paragraph}"
        );
    }
}

/// The Licence section says whose the artwork in each folder of it is, by naming the folder: what
/// the original's repository gave and what CrossFX drew are told apart folder by folder, not
/// given to one of them as a whole.
#[test]
fn the_readme_names_every_folder_of_artwork_the_tree_ships() {
    let mut folders: Vec<String> = Vec::new();
    for file in artwork() {
        let folder = file.rsplit_once('/').map_or("", |(folder, _)| folder);
        if !folders.iter().any(|known| known == folder) {
            folders.push(folder.to_owned());
        }
    }
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

/// `assets/images` holds the original's images and the ones CrossFX drew to match beside each
/// other, so its name cannot say whose each one is: the section gives the folder to FxSound LLC
/// and names, as the exceptions, exactly the images CrossFX drew. Every image there that is not
/// one of `ORIGINAL_IMAGES` has to be named, and none of those; and each of those has to be
/// there, so that the list stays the folder's.
#[test]
fn the_readme_names_each_image_crossfx_drew_and_none_of_the_originals() {
    let readme = text("README.md");
    let section = licence_section(&readme);
    let images: Vec<String> = artwork()
        .iter()
        .filter_map(|path| path.strip_prefix("assets/images/"))
        .map(str::to_owned)
        .collect();
    for original in ORIGINAL_IMAGES {
        assert!(
            images.iter().any(|image| image == original),
            "assets/images/{original} is gone: take it off ORIGINAL_IMAGES"
        );
        assert!(
            !section.contains(&format!("`{original}`")),
            "README.md's Licence section names {original}, the original's, among the images \
             CrossFX drew:\n{section}"
        );
    }
    for image in images
        .iter()
        .filter(|image| !ORIGINAL_IMAGES.contains(&image.as_str()))
    {
        assert!(
            section.contains(&format!("`{image}`")),
            "assets/images/{image} is not one of the original's files, and README.md's Licence \
             section does not name it among the images CrossFX drew (were it the original's, it \
             would belong in ORIGINAL_IMAGES):\n{section}"
        );
    }
}

/// `debian/copyright`, which a `.deb`'s packager reads in place of the README, gives the artwork
/// the holders the section gives it: FxSound LLC each of the original's images, and CrossFX's
/// contributors alone everything else, the images CrossFX drew and its own icon.
#[test]
fn debian_copyright_gives_the_artwork_the_holders_the_readme_gives_it() {
    let stanzas = stanzas(&text("packaging/debian/copyright"));
    let mut wrong = Vec::new();
    for path in artwork() {
        let holders = holders(&stanzas, &path);
        let original = path
            .strip_prefix("assets/images/")
            .is_some_and(|name| ORIGINAL_IMAGES.contains(&name));
        let right = if original {
            holders.contains("FxSound LLC")
        } else {
            holders.contains("CrossFX contributors") && !holders.contains("FxSound LLC")
        };
        if !right {
            let whose = if original {
                "the original's"
            } else {
                "CrossFX's own"
            };
            wrong.push(format!("{path}, {whose}: {}", holders.replace('\n', "; ")));
        }
    }
    assert!(
        wrong.is_empty(),
        "debian/copyright gives this artwork to the wrong holders:\n{}",
        wrong.join("\n")
    );
}

/// Every `.rs` file under `dir`, read into one string.
fn rust_sources(dir: &Path, into: &mut String) {
    for entry in fs::read_dir(dir).expect("the directory is listed") {
        let path = entry.expect("an entry").path();
        if path.is_dir() {
            rust_sources(&path, into);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            into.push_str(&fs::read_to_string(&path).unwrap_or_default());
        }
    }
}

/// The tree ships no artwork nothing uses: each image of `assets/images` is named by the source
/// of one of the crates (the interface embeds them with `include_bytes!`), so that the original's
/// button states and equalizer glyph, which CrossFX never drew with, are not carried for nothing.
#[test]
fn every_image_in_assets_images_is_used_by_the_source() {
    let mut source = String::new();
    for crate_dir in fs::read_dir(repo().join("crates")).expect("crates is listed") {
        let src = crate_dir.expect("an entry").path().join("src");
        if src.is_dir() {
            rust_sources(&src, &mut source);
        }
    }
    let unused: Vec<String> = artwork()
        .iter()
        .filter_map(|path| path.strip_prefix("assets/images/"))
        .filter(|name| {
            name.ends_with(".svg")
                && !(source.contains(&format!("\"{name}\""))
                    || source.contains(&format!("/{name}\"")))
        })
        .map(str::to_owned)
        .collect();
    assert!(
        unused.is_empty(),
        "assets/images holds images no crate refers to; remove them: {unused:?}"
    );
}

/// The notices of the Rust crates compiled into the binary are a file of the tree, which every
/// package installs, and the Licence section says so: what the file is, that it is generated from
/// `Cargo.lock` by a test, and the command that rewrites it. The archive's and the packages' lists
/// name it too.
#[test]
fn the_readme_names_the_generated_notices_of_the_crates_and_how_they_are_kept_current() {
    assert!(
        notices().iter().any(|notice| notice == THIRD_PARTY),
        "packaging/build-tarball.sh no longer installs {THIRD_PARTY}"
    );
    assert!(
        !text(THIRD_PARTY).trim().is_empty(),
        "{THIRD_PARTY} is empty"
    );
    let readme = text("README.md");
    let section = licence_section(&readme);
    let flat = flowing(section);
    for needed in [
        format!("`{THIRD_PARTY}`"),
        "crates/crossfx-app/tests/third_party_notices.rs".to_string(),
        "CROSSFX_WRITE_NOTICES=1 cargo test -p crossfx-app --test third_party_notices".to_string(),
    ] {
        assert!(
            flat.contains(&needed),
            "README.md's Licence section does not mention {needed}:\n{section}"
        );
    }
    let archive = readme
        .split_once("`share/doc/crossfx/` (")
        .and_then(|(_, rest)| rest.split_once(')'))
        .map(|(list, _)| list)
        .expect("README.md says what the archive's share/doc/crossfx holds");
    let packages = readme
        .lines()
        .find(|line| line.starts_with("| `/usr/share/licenses/crossfx/` |"))
        .expect("README.md says what /usr/share/licenses/crossfx holds");
    for (what, list) in [
        ("the archive's share/doc/crossfx", archive),
        ("/usr/share/licenses/crossfx", packages),
    ] {
        assert!(
            list.contains(THIRD_PARTY),
            "README.md leaves {THIRD_PARTY} out of what {what} holds: {list}"
        );
    }
}
