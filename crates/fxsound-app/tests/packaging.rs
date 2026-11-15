//! What the packages ship, held by running the recipes themselves wherever that needs no package
//! manager: `packaging/build-tarball.sh` with a stand-in for cargo, and the `package()` function of
//! each Arch recipe in a plain bash, the way makepkg calls it. What cannot be run here — an RPM
//! build, dpkg — is read: the Fedora spec's `%files` and Debian's machine-readable `copyright`.
//!
//! Two things are held. The first is the licences. The binary carries three parts that are not
//! AGPL: the RNNoise code and model in `crates/fxsound-rnnoise` (BSD-3-Clause, whose second clause
//! makes reproducing the notice in the accompanying materials the condition of shipping a binary),
//! and the Noto fallback faces and the Manrope interface faces embedded in the window (SIL OFL 1.1,
//! whose second condition asks the same of each font's notice and licence). 0.4.0's first packages
//! installed only `LICENSE`, and Debian's `copyright` called every file in the tree AGPL, fonts
//! included. The second is that the tarball script packs the binary of the tree it sits in: it
//! used to build only when there was no binary at all, so a 0.3.0 left in `target/release` went
//! out under the 0.4.0 name. And the tarball's `install.sh` refuses a prefix inside a home folder,
//! where its copy would lay the factory presets over the user's own.
//!
//! The recipes and files are packaging's, under CrossFX's names (`crossfx`, `crossfx-bin`,
//! `crossfx.spec`, `debian/crossfx.install`): a test that runs or reads one carries the rename's
//! marker until the integrator has merged them.

// The Linux packages' recipes, run in a POSIX shell with Unix permissions.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::process::Output;

use fxsound_core::test_support::{ScratchDir, command};

/// The repository this test binary was built from.
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root exists")
}

fn read(path: impl AsRef<Path>) -> Vec<u8> {
    let path = path.as_ref();
    fs::read(path).unwrap_or_else(|error| panic!("{} could not be read: {error}", path.display()))
}

fn text(path: impl AsRef<Path>) -> String {
    String::from_utf8(read(path)).expect("the file is UTF-8")
}

fn write_executable(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().expect("a file has a parent")).expect("the parent is made");
    fs::write(path, contents).expect("the file is written");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("it is made executable");
}

/// What a run printed, for a failure message.
fn transcript(output: &Output) -> String {
    format!(
        "status {}\n--- stdout\n{}--- stderr\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The three notices every package has to carry beside `LICENSE`, as (installed name, source).
fn notices() -> [(&'static str, PathBuf); 3] {
    [
        (
            "COPYING.rnnoise",
            repo().join("crates/fxsound-rnnoise/COPYING"),
        ),
        ("OFL.noto", repo().join("assets/fonts/OFL.txt")),
        ("OFL.manrope", repo().join("assets/fonts/OFL-Manrope.txt")),
    ]
}

/// A copy of the source tree made of links to the real one, with a `Cargo.toml` of its own (for
/// the version) and a `target/`, `dist/` and temporary directory the runs may write into. Nothing
/// a run writes can reach the repository: every directory it writes in is a real one in here.
struct Tree {
    scratch: ScratchDir,
}

/// The version the tree's `Cargo.toml` is at.
const TREE_VERSION: &str = "9.8.7";

impl Tree {
    fn new(tag: &str) -> Self {
        let scratch = ScratchDir::new(&format!("packaging-{tag}"));
        let tree = Self { scratch };
        let root = tree.root();
        let repo = repo();
        // A real directory of linked files rather than a linked directory, so that the script's
        // `cd "$(dirname "$0")/.."` lands in this tree and not in the repository.
        fs::create_dir_all(root.join("packaging")).expect("packaging/ is made");
        for entry in fs::read_dir(repo.join("packaging")).expect("packaging/ is listed") {
            let entry = entry.expect("an entry of packaging/");
            symlink(entry.path(), root.join("packaging").join(entry.file_name()))
                .expect("a packaging file is linked");
        }
        for shared in ["assets", "crates", "README.md", "CHANGELOG.md", "LICENSE"] {
            symlink(repo.join(shared), root.join(shared)).expect("a source path is linked");
        }
        fs::write(
            root.join("Cargo.toml"),
            format!("[workspace.package]\nversion = \"{TREE_VERSION}\"\n"),
        )
        .expect("Cargo.toml is written");
        fs::create_dir_all(tree.scratch.join("tmp")).expect("a temporary directory is made");
        // Stands in for cargo: notes how it was called and, told a version, "builds" a binary that
        // reports it, where a real build would put it.
        write_executable(
            &tree.scratch.join("bin/cargo"),
            r#"#!/bin/sh
printf '%s | CARGO_TARGET_DIR=%s\n' "$*" "${CARGO_TARGET_DIR:-}" >> "$STUB_CARGO_LOG"
if [ -n "${STUB_BUILDS:-}" ]; then
  out="${CARGO_TARGET_DIR:-target}/release"
  mkdir -p "$out"
  printf '#!/bin/sh\necho "crossfx %s"\n# built by this run\n' "$STUB_BUILDS" > "$out/crossfx"
  chmod 755 "$out/crossfx"
fi
"#,
        );
        tree
    }

    fn root(&self) -> PathBuf {
        self.scratch.join("root")
    }

    /// Leave a binary in `target/release` that says it is `version`, as a build of another
    /// checkout would.
    fn leave_binary(&self, version: &str, marker: &str) {
        write_executable(
            &self.root().join("target/release/crossfx"),
            &format!("#!/bin/sh\necho \"crossfx {version}\"\n# {marker}\n"),
        );
    }

    /// Run `packaging/build-tarball.sh` in this tree. `builds` is the version the stand-in cargo
    /// builds, or `None` for a cargo that leaves `target/` alone; `target_dir` is a
    /// `CARGO_TARGET_DIR` the caller's shell has set.
    fn build_tarball(&self, builds: Option<&str>, target_dir: Option<&Path>) -> Output {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![self.scratch.join("bin")];
        paths.extend(std::env::split_paths(&path));
        let mut run = command("bash");
        run.arg(self.root().join("packaging/build-tarball.sh"))
            .current_dir(self.scratch.path())
            .env("PATH", std::env::join_paths(paths).expect("PATH is joined"))
            .env("TMPDIR", self.scratch.join("tmp"))
            .env("STUB_CARGO_LOG", self.cargo_log_path())
            .env_remove("STUB_BUILDS")
            .env_remove("CARGO_TARGET_DIR");
        if let Some(version) = builds {
            run.env("STUB_BUILDS", version);
        }
        if let Some(dir) = target_dir {
            run.env("CARGO_TARGET_DIR", dir);
        }
        run.output().expect("bash runs")
    }

    fn cargo_log_path(&self) -> PathBuf {
        self.scratch.join("cargo.log")
    }

    fn cargo_log(&self) -> String {
        fs::read_to_string(self.cargo_log_path()).unwrap_or_default()
    }

    /// The archives in `dist/`.
    fn tarballs(&self) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(self.root().join("dist")) else {
            return Vec::new();
        };
        entries
            .map(|entry| entry.expect("an entry of dist/").path())
            .filter(|path| path.to_string_lossy().ends_with(".tar.gz"))
            .collect()
    }

    /// Unpack the one archive in `dist/` into `into`, without its top directory.
    fn unpack_tarball(&self, into: &Path) {
        let tarballs = self.tarballs();
        let [tarball] = tarballs.as_slice() else {
            panic!("dist/ should hold one archive, and holds {tarballs:?}");
        };
        fs::create_dir_all(into).expect("the directory to unpack into is made");
        let unpacked = command("tar")
            .arg("-xzf")
            .arg(tarball)
            .arg("--strip-components=1")
            .arg("-C")
            .arg(into)
            .output()
            .expect("tar runs");
        assert!(unpacked.status.success(), "{}", transcript(&unpacked));
    }

    /// Source `recipe` in bash and call its `package()`, as makepkg does, with `srcdir` and
    /// `startdir` as given and `pkgdir` a fresh directory, which is returned. `before` runs
    /// between the two, with the recipe's variables set and `$UNPACKED` naming `unpacked`. Also
    /// returns the recipe's `license` array.
    fn run_package(
        &self,
        recipe: &Path,
        srcdir: &Path,
        startdir: &Path,
        (before, unpacked): (&str, &Path),
    ) -> (PathBuf, Vec<String>) {
        let pkgdir = self.scratch.join("pkg");
        let script = format!(
            "set -e\nsource \"$RECIPE\"\n{before}\npackage\nprintf '%s\\n' \"${{license[@]}}\" \
             > \"$LICENSES\"\n"
        );
        let licenses = self.scratch.join("licenses.txt");
        let packaged = command("bash")
            .arg("-c")
            .arg(script)
            .current_dir(self.scratch.path())
            .env("RECIPE", recipe)
            .env("srcdir", srcdir)
            .env("startdir", startdir)
            .env("pkgdir", &pkgdir)
            .env("LICENSES", &licenses)
            .env("UNPACKED", unpacked)
            .output()
            .expect("bash runs");
        assert!(
            packaged.status.success(),
            "{}'s package() failed: {}",
            recipe.display(),
            transcript(&packaged)
        );
        let declared = text(&licenses).lines().map(str::to_owned).collect();
        (pkgdir, declared)
    }
}

/// `dir` holds `LICENSE` and both notices, each byte for byte what the tree has.
fn assert_notices_in(dir: &Path, package: &str) {
    assert_eq!(
        read(dir.join("LICENSE")),
        read(repo().join("LICENSE")),
        "{package}: {}/LICENSE is not the tree's LICENSE",
        dir.display()
    );
    for (name, source) in notices() {
        let installed = dir.join(name);
        assert!(
            installed.is_file(),
            "{package} does not install {name} ({}) into {}",
            source.display(),
            dir.display()
        );
        assert_eq!(
            read(&installed),
            read(&source),
            "{package}: {} is not {}",
            installed.display(),
            source.display()
        );
    }
}

/// An Arch recipe that installs the BSD and OFL texts says it ships those licences, too.
fn assert_declares_the_notices(declared: &[String], package: &str) {
    for licence in ["AGPL-3.0-or-later", "BSD-3-Clause", "OFL-1.1"] {
        assert!(
            declared.iter().any(|declared| declared == licence),
            "{package}'s license=() is {declared:?}, without {licence}"
        );
    }
}

#[test]
#[ignore = "crossfx-rename: until integration"]
fn the_tarball_script_builds_the_tree_it_sits_in_even_when_a_binary_is_already_there() {
    let tree = Tree::new("tarball-rebuilds");
    // A binary from an older checkout, and a shell that sends cargo's output somewhere else: the
    // script has to build anyway, and into the target/ it packs from.
    tree.leave_binary("0.3.0", "left by an older checkout");
    let elsewhere = tree.scratch.join("elsewhere");

    let run = tree.build_tarball(Some(TREE_VERSION), Some(&elsewhere));

    assert!(run.status.success(), "{}", transcript(&run));
    let log = tree.cargo_log();
    assert!(
        log.lines().any(|call| call.starts_with("build ")
            && call.contains("--release")
            && call.contains("--bin crossfx")),
        "the script did not build the binary; cargo was called as:\n{log}"
    );
    let unpacked = tree.scratch.join("unpacked");
    tree.unpack_tarball(&unpacked);
    let packed = text(unpacked.join("bin/crossfx"));
    assert!(
        packed.contains("built by this run") && packed.contains(TREE_VERSION),
        "the archive holds the binary that was there before the build:\n{packed}"
    );
}

#[test]
#[ignore = "crossfx-rename: until integration"]
fn the_tarball_script_refuses_a_binary_that_is_not_the_trees_version() {
    let tree = Tree::new("tarball-refuses");
    tree.leave_binary("0.3.0", "left by an older checkout");

    // A build that did not replace it.
    let run = tree.build_tarball(None, None);

    assert!(
        !run.status.success(),
        "a 0.3.0 binary was packed for a {TREE_VERSION} tree: {}",
        transcript(&run)
    );
    let said = String::from_utf8_lossy(&run.stderr);
    assert!(
        said.contains("0.3.0") && said.contains(TREE_VERSION),
        "the refusal should name both versions: {}",
        transcript(&run)
    );
    assert!(
        tree.tarballs().is_empty(),
        "an archive was written anyway: {:?}",
        tree.tarballs()
    );
}

#[test]
#[ignore = "crossfx-rename: until integration"]
fn the_tarball_and_the_bin_package_made_from_it_carry_the_notices_of_rnnoise_and_noto() {
    let tree = Tree::new("tarball-notices");
    let run = tree.build_tarball(Some(TREE_VERSION), None);
    assert!(run.status.success(), "{}", transcript(&run));

    // The tarball, as install.sh copies it: share/doc/crossfx.
    let unpacked = tree.scratch.join("unpacked");
    tree.unpack_tarball(&unpacked);
    assert_notices_in(&unpacked.join("share/doc/crossfx"), "the tarball");

    // crossfx-bin, which is that tarball as makepkg unpacks it into $srcdir.
    let srcdir = tree.scratch.join("src");
    let (pkgdir, declared) = tree.run_package(
        &repo().join("packaging/aur/crossfx-bin/PKGBUILD"),
        &srcdir,
        &srcdir,
        (
            "mkdir -p \"$srcdir/${_pkgname}-${pkgver}-x86_64\"\n\
             cp -a \"$UNPACKED\"/. \"$srcdir/${_pkgname}-${pkgver}-x86_64\"/",
            &unpacked,
        ),
    );
    assert_notices_in(&pkgdir.join("usr/share/licenses/crossfx"), "crossfx-bin");
    assert_declares_the_notices(&declared, "crossfx-bin");
}

#[test]
#[ignore = "crossfx-rename: until integration"]
fn the_arch_package_installs_the_notices_of_rnnoise_and_noto_beside_its_licence() {
    let tree = Tree::new("arch-notices");
    tree.leave_binary(TREE_VERSION, "built from this tree");
    let startdir = tree.root().join("packaging");

    let (pkgdir, declared) = tree.run_package(
        &startdir.join("PKGBUILD"),
        &tree.scratch.join("src"),
        &startdir,
        ("", Path::new("")),
    );

    assert_notices_in(
        &pkgdir.join("usr/share/licenses/crossfx"),
        "packaging/PKGBUILD",
    );
    assert_declares_the_notices(&declared, "packaging/PKGBUILD");
}

/// The body of the spec section that starts with `header` (`%install`, `%files`): up to the next
/// line that starts a section.
fn spec_section<'a>(spec: &'a str, header: &str) -> Vec<&'a str> {
    const SECTIONS: [&str; 10] = [
        "%prep",
        "%build",
        "%install",
        "%check",
        "%post",
        "%preun",
        "%postun",
        "%files",
        "%changelog",
        "%description",
    ];
    spec.lines()
        .skip_while(|line| line.trim_end() != header)
        .skip(1)
        .take_while(|line| {
            !SECTIONS
                .iter()
                .any(|section| line.split_whitespace().next() == Some(section))
        })
        .collect()
}

#[test]
#[ignore = "crossfx-rename: until integration"]
fn the_fedora_package_ships_the_notices_of_rnnoise_and_noto_as_licence_files() {
    let spec = text(repo().join("packaging/fedora/crossfx.spec"));
    let install = spec_section(&spec, "%install");
    let files = spec_section(&spec, "%files");
    for (name, source) in notices() {
        let source = source
            .strip_prefix(repo())
            .expect("the notice is in the tree")
            .display()
            .to_string();
        // Under its own name, so that it does not land in %{_licensedir} as a bare COPYING.
        assert!(
            install.iter().any(|line| {
                let words: Vec<&str> = line.split_whitespace().collect();
                words.first() == Some(&"cp")
                    && words.contains(&source.as_str())
                    && words.last() == Some(&name)
            }),
            "the spec's %install does not copy {source} to {name}"
        );
        assert!(
            files
                .iter()
                .any(|line| line.split_whitespace().collect::<Vec<_>>() == ["%license", name]),
            "the spec's %files has no `%license {name}`"
        );
    }
}

/// One paragraph of a machine-readable debian/copyright: its fields, continuation lines joined
/// with newlines (a lone `.` is an empty line).
struct Paragraph(Vec<(String, String)>);

impl Paragraph {
    fn field(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.as_str())
    }
}

fn dep5(copyright: &str) -> Vec<Paragraph> {
    let mut paragraphs = Vec::new();
    let mut fields: Vec<(String, String)> = Vec::new();
    for line in copyright.lines().chain([""]) {
        if line.trim().is_empty() {
            if !fields.is_empty() {
                paragraphs.push(Paragraph(std::mem::take(&mut fields)));
            }
        } else if let Some(continued) = line.strip_prefix(' ') {
            let (_, value) = fields.last_mut().expect("a continuation follows a field");
            value.push('\n');
            if continued.trim() != "." {
                value.push_str(continued);
            }
        } else {
            let (name, value) = line.split_once(':').expect("a field has a colon");
            fields.push((name.to_owned(), value.trim().to_owned()));
        }
    }
    paragraphs
}

/// A DEP-5 `Files:` pattern: `*` is any run of characters, `/` included, and `?` any one.
fn matches(pattern: &[u8], path: &[u8]) -> bool {
    match (pattern.split_first(), path.split_first()) {
        (None, None) => true,
        (Some((b'*', rest)), _) => {
            matches(rest, path) || (!path.is_empty() && matches(pattern, &path[1..]))
        }
        (Some((b'?', rest)), Some((_, tail))) => matches(rest, tail),
        (Some((expected, rest)), Some((found, tail))) => expected == found && matches(rest, tail),
        _ => false,
    }
}

/// The licence debian/copyright gives `path`: that of the last `Files:` paragraph that matches
/// it, which is how the format resolves overlaps.
fn licence_of<'a>(paragraphs: &'a [Paragraph], path: &str) -> &'a str {
    paragraphs
        .iter()
        .rev()
        .find(|paragraph| {
            paragraph.field("Files").is_some_and(|files| {
                files
                    .split_whitespace()
                    .any(|pattern| matches(pattern.as_bytes(), path.as_bytes()))
            })
        })
        .and_then(|paragraph| paragraph.field("License"))
        .map(|licence| licence.lines().next().unwrap_or_default())
        .unwrap_or_else(|| panic!("no Files paragraph of debian/copyright covers {path}"))
}

/// Every file under `dir`, as a path relative to the repository.
fn files_under(dir: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut pending = vec![repo().join(dir)];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).expect("the directory is listed") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let relative = path.strip_prefix(repo()).expect("under the repository");
                found.push(relative.display().to_string());
            }
        }
    }
    found.sort();
    found
}

#[test]
fn debian_copyright_gives_the_fonts_and_the_vendored_rnnoise_the_licences_they_carry() {
    let copyright = text(repo().join("packaging/debian/copyright"));
    let paragraphs = dep5(&copyright);

    let mut wrong = Vec::new();
    let mut expect = |path: &str, licence: &str| {
        let found = licence_of(&paragraphs, path);
        if found != licence {
            wrong.push(format!("{path}: {found}, should be {licence}"));
        }
    };
    for path in files_under("crates/fxsound-rnnoise") {
        expect(&path, "BSD-3-Clause");
    }
    for path in files_under("assets/fonts") {
        let name = path.rsplit('/').next().expect("a file name");
        if name.starts_with("Noto")
            || name == "OFL.txt"
            || name.starts_with("Manrope-")
            || name == "OFL-Manrope.txt"
        {
            expect(&path, "OFL-1.1");
        } else {
            expect(&path, "a licence this test has been told of");
        }
    }
    // And CrossFX itself is still the AGPL.
    expect("crates/fxsound-dsp/src/lib.rs", "AGPL-3.0-or-later");
    expect("assets/presets/Input/Broadcaster.toml", "AGPL-3.0-or-later");
    expect("debian/rules", "AGPL-3.0-or-later");

    assert!(
        wrong.is_empty(),
        "debian/copyright says:\n{}",
        wrong.join("\n")
    );

    // Every licence a Files paragraph names has its text: in the paragraph, or in a License
    // paragraph of its own.
    for paragraph in paragraphs.iter().filter(|p| p.field("Files").is_some()) {
        let licence = paragraph
            .field("License")
            .expect("a Files paragraph has a License");
        let name = licence.lines().next().unwrap_or_default();
        let inline = licence.lines().count() > 1;
        let standalone = paragraphs.iter().any(|other| {
            other.field("Files").is_none()
                && other.field("License").is_some_and(|text| {
                    text.lines().next() == Some(name) && text.lines().count() > 1
                })
        });
        assert!(
            inline || standalone,
            "debian/copyright names {name} without its text"
        );
    }
}

/// The text of a licence, whitespace aside, for comparing a copy with its source.
fn words(text: &str) -> Vec<&str> {
    text.split_whitespace().collect()
}

#[test]
fn debian_copyright_reproduces_the_rnnoise_notice_and_the_ofl_word_for_word() {
    let copyright = text(repo().join("packaging/debian/copyright"));
    let paragraphs = dep5(&copyright);
    let standalone = |name: &str| -> String {
        let licence = paragraphs
            .iter()
            .filter(|paragraph| paragraph.field("Files").is_none())
            .filter_map(|paragraph| paragraph.field("License"))
            .find(|licence| licence.lines().next() == Some(name))
            .unwrap_or_else(|| panic!("debian/copyright has no License: {name} paragraph"));
        licence
            .split_once('\n')
            .map(|(_, text)| text)
            .unwrap_or_default()
            .to_owned()
    };

    // BSD-3-Clause: the conditions and disclaimer of the vendored COPYING, and every holder it
    // names on the rnnoise paragraph's Copyright.
    let copying = text(repo().join("crates/fxsound-rnnoise/COPYING"));
    let (holders, conditions) = copying
        .split_once("Redistribution and use")
        .expect("COPYING states its conditions");
    assert_eq!(
        words(&standalone("BSD-3-Clause")),
        words(&format!("Redistribution and use{conditions}")),
        "debian/copyright's BSD-3-Clause text is not crates/fxsound-rnnoise/COPYING's"
    );
    let rnnoise = paragraphs
        .iter()
        .find(|paragraph| paragraph.field("Files") == Some("crates/fxsound-rnnoise/*"))
        .expect("debian/copyright has a crates/fxsound-rnnoise/* paragraph");
    let named = rnnoise.field("Copyright").unwrap_or_default();
    for holder in holders
        .lines()
        .filter_map(|line| line.strip_prefix("Copyright (c) "))
    {
        let (years, who) = holder.split_once(", ").expect("years, then the holder");
        assert!(
            named
                .lines()
                .any(|line| line.trim() == format!("{years} {who}")),
            "the rnnoise paragraph's Copyright does not name {years} {who}"
        );
    }

    // OFL-1.1: the licence as assets/fonts/OFL.txt carries it, from its first rule down.
    let ofl = text(repo().join("assets/fonts/OFL.txt"));
    let licence = &ofl[ofl
        .find("-----------------------------------------------------------\nSIL OPEN FONT")
        .expect("OFL.txt carries the licence")..];
    assert_eq!(
        words(&standalone("OFL-1.1")),
        words(licence),
        "debian/copyright's OFL-1.1 text is not assets/fonts/OFL.txt's"
    );
    // Manrope's upstream notice carries the same licence, so the one paragraph serves both.
    let manrope = text(repo().join("assets/fonts/OFL-Manrope.txt"));
    let manrope_licence = &manrope[manrope
        .find("-----------------------------------------------------------\nSIL OPEN FONT")
        .expect("OFL-Manrope.txt carries the licence")..];
    assert_eq!(
        words(manrope_licence),
        words(licence),
        "assets/fonts/OFL-Manrope.txt's licence is not the OFL 1.1 of OFL.txt"
    );
}

#[test]
fn the_tree_carries_manropes_notice_and_no_gilroy() {
    // Gilroy is Radomir Tinkov's commercial typeface: CrossFX neither ships nor downloads it, and
    // the window uses it only when the user adds files they licensed (fxsound-app's `fonts`).
    let notice = text(repo().join("assets/fonts/OFL-Manrope.txt"));
    assert!(
        notice.starts_with("Copyright 2018 The Manrope Project Authors"),
        "{notice:.80}"
    );
    let fonts = files_under("assets/fonts");
    for face in ["Regular", "SemiBold", "Bold"] {
        let path = format!("assets/fonts/Manrope-{face}.ttf");
        assert!(fonts.contains(&path), "{path} is not in the tree");
    }
    let gilroy: Vec<String> = files_under("assets")
        .into_iter()
        .filter(|path| {
            let name = path.rsplit('/').next().unwrap_or_default().to_lowercase();
            name.contains("gilroy") && (name.ends_with(".ttf") || name.ends_with(".otf"))
        })
        .collect();
    assert!(gilroy.is_empty(), "Gilroy files in the tree: {gilroy:?}");
}

#[test]
fn the_noto_notice_covers_every_noto_face_in_the_tree() {
    let notice = text(repo().join("assets/fonts/OFL.txt"));
    for path in files_under("assets/fonts") {
        let name = path.rsplit('/').next().expect("a file name");
        if !name.starts_with("Noto") {
            continue;
        }
        // NotoSansKR-Bold.otf is covered by "NotoSansKR-*.otf".
        let (family, rest) = name.split_once('-').expect("family-weight.extension");
        let extension = rest.rsplit('.').next().expect("an extension");
        let pattern = format!("{family}-*.{extension}");
        assert!(
            notice.contains(&pattern),
            "assets/fonts/OFL.txt does not give the notice of {name} (no {pattern} in it)"
        );
    }
}

/// The application icon's one name: the application id, which the desktop entry's `Icon=` names
/// and every notification is sent with (`crates/fxsound-app/src/notify.rs`). A notification
/// daemon resolves it through the icon theme: an icon no package shipped under it had Noctalia log
/// "theme icon not found" for each notice.
const NOTIFICATION_ICON: &str = fxsound_app::tray::APP_ID;

/// Each size of the application icon under `share/icons/hicolor`, as (size, source in the tree):
/// the five bars of the original's artwork, whose file names stay.
const APPLICATION_ICONS: [(&str, &str); 2] = [
    ("256x256", "assets/images/fxsound_large.png"),
    ("32x32", "assets/images/fxsound.png"),
];

/// `share` holds the application icon at every size under its one name, the application id, byte
/// for byte the tree's file.
fn assert_notification_icon_in(share: &Path, package: &str) {
    for (size, source) in APPLICATION_ICONS {
        let installed = share.join(format!("icons/hicolor/{size}/apps/{NOTIFICATION_ICON}.png"));
        assert!(
            installed.is_file(),
            "{package} does not install {}",
            installed.display()
        );
        assert_eq!(
            read(&installed),
            read(repo().join(source)),
            "{package}: {} is not {source}",
            installed.display()
        );
    }
}

/// The shell commands of a script or a spec section, each with its `\` continuations joined into
/// one line of words.
fn shell_commands<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<Vec<String>> {
    let mut commands = Vec::new();
    let mut current = String::new();
    for line in lines {
        let line = line.trim();
        match line.strip_suffix('\\') {
            Some(start) => {
                current.push_str(start);
                current.push(' ');
            }
            None => {
                current.push_str(line);
                commands.push(current.split_whitespace().map(str::to_owned).collect());
                current.clear();
            }
        }
    }
    commands
}

/// Whether one of `commands` is an `install` of a file whose path ends in `source` to a path that
/// ends in `destination`.
fn installs(commands: &[Vec<String>], source: &str, destination: &str) -> bool {
    commands.iter().any(|words| {
        words.first().map(String::as_str) == Some("install")
            && words
                .iter()
                .any(|word| word.trim_matches('"').ends_with(source))
            && words
                .last()
                .is_some_and(|word| word.trim_matches('"').ends_with(destination))
    })
}

#[test]
#[ignore = "crossfx-rename: until integration"]
fn the_tarball_and_the_bin_package_made_from_it_install_the_notification_icon_under_the_application_id()
 {
    let tree = Tree::new("tarball-icon");
    let run = tree.build_tarball(Some(TREE_VERSION), None);
    assert!(run.status.success(), "{}", transcript(&run));

    let unpacked = tree.scratch.join("unpacked");
    tree.unpack_tarball(&unpacked);
    assert_notification_icon_in(&unpacked.join("share"), "the tarball");

    let srcdir = tree.scratch.join("src");
    let (pkgdir, _) = tree.run_package(
        &repo().join("packaging/aur/crossfx-bin/PKGBUILD"),
        &srcdir,
        &srcdir,
        (
            "mkdir -p \"$srcdir/${_pkgname}-${pkgver}-x86_64\"\n\
             cp -a \"$UNPACKED\"/. \"$srcdir/${_pkgname}-${pkgver}-x86_64\"/",
            &unpacked,
        ),
    );
    assert_notification_icon_in(&pkgdir.join("usr/share"), "crossfx-bin");
}

#[test]
#[ignore = "crossfx-rename: until integration"]
fn the_arch_package_installs_the_notification_icon_under_the_application_id() {
    let tree = Tree::new("arch-icon");
    tree.leave_binary(TREE_VERSION, "built from this tree");
    let startdir = tree.root().join("packaging");

    let (pkgdir, _) = tree.run_package(
        &startdir.join("PKGBUILD"),
        &tree.scratch.join("src"),
        &startdir,
        ("", Path::new("")),
    );

    assert_notification_icon_in(&pkgdir.join("usr/share"), "packaging/PKGBUILD");
}

#[test]
#[ignore = "crossfx-rename: until integration"]
fn the_fedora_package_installs_and_lists_the_notification_icon_under_the_application_id() {
    let spec = text(repo().join("packaging/fedora/crossfx.spec"));
    let install = shell_commands(spec_section(&spec, "%install"));
    let files = spec_section(&spec, "%files");
    for (size, source) in APPLICATION_ICONS {
        let path = format!("%{{_datadir}}/icons/hicolor/{size}/apps/{NOTIFICATION_ICON}.png");
        assert!(
            installs(&install, source, &path),
            "the spec's %install does not install {source} as {path}"
        );
        assert!(
            files.iter().any(|line| line.trim() == path),
            "the spec's %files does not list {path}"
        );
    }
}

#[test]
#[ignore = "crossfx-rename: until integration"]
fn both_debian_builds_install_the_notification_icon_under_the_application_id() {
    for recipe in ["packaging/debian/rules", "packaging/build-deb.sh"] {
        let commands = shell_commands(text(repo().join(recipe)).lines());
        for (size, source) in APPLICATION_ICONS {
            let path = format!("/usr/share/icons/hicolor/{size}/apps/{NOTIFICATION_ICON}.png");
            assert!(
                installs(&commands, source, &path),
                "{recipe} does not install {source} as {path}"
            );
        }
    }
    // debian/rules installs into debian/tmp; the .install file is what moves it into the package.
    let moved = text(repo().join("packaging/debian/crossfx.install"));
    assert!(
        moved
            .lines()
            .any(|line| line.trim() == "usr/share/icons/hicolor"),
        "crossfx.install does not carry usr/share/icons/hicolor into the package"
    );
}

/// The tarball's `install.sh` (`packaging/build-tarball.sh` writes it), unpacked from an archive
/// of this tree, and folders of the run's own beside it.
struct Installer {
    tree: Tree,
    unpacked: PathBuf,
}

impl Installer {
    fn new(tag: &str) -> Self {
        let tree = Tree::new(tag);
        let run = tree.build_tarball(Some(TREE_VERSION), None);
        assert!(run.status.success(), "{}", transcript(&run));
        let unpacked = tree.scratch.join("unpacked");
        tree.unpack_tarball(&unpacked);
        Self { tree, unpacked }
    }

    fn script(&self) -> PathBuf {
        self.unpacked.join("install.sh")
    }

    /// A folder of the run's own in the scratch directory, made, by the path the shell resolves
    /// it to.
    fn dir(&self, name: &str) -> PathBuf {
        let dir = self.tree.scratch.join(name);
        fs::create_dir_all(&dir).expect("the folder is made");
        dir.canonicalize().expect("the folder resolves")
    }
}

/// What the shell `install.sh` runs in says: the home, the data home, the user `sudo` was run by,
/// and a folder of stand-ins put first on `PATH`. Of these, nothing of the test's own environment
/// reaches it.
struct Shell<'a> {
    home: &'a Path,
    data_home: Option<&'a Path>,
    sudo_user: Option<&'a str>,
    stand_ins: Option<&'a Path>,
}

/// `sh install.sh <prefix>`, as a user runs it from the unpacked tarball.
fn run_install(script: &Path, prefix: &Path, shell: &Shell<'_>) -> Output {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths: Vec<PathBuf> = shell
        .stand_ins
        .iter()
        .map(|dir| dir.to_path_buf())
        .collect();
    paths.extend(std::env::split_paths(&path));
    let mut run = command("sh");
    run.arg(script)
        .arg(prefix)
        .env("PATH", std::env::join_paths(paths).expect("PATH is joined"))
        .env("HOME", shell.home)
        .env_remove("XDG_DATA_HOME")
        .env_remove("SUDO_USER");
    if let Some(data_home) = shell.data_home {
        run.env("XDG_DATA_HOME", data_home);
    }
    if let Some(user) = shell.sudo_user {
        run.env("SUDO_USER", user);
    }
    run.output().expect("sh runs")
}

/// Every folder and file under `dir`, each file with its bytes: what a refused run must leave as
/// it found it.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).expect("the folder is listed") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                found.push((path.clone(), None));
                pending.push(path);
            } else {
                let bytes = read(&path);
                found.push((path, Some(bytes)));
            }
        }
    }
    found.sort();
    found
}

/// The run changed nothing and said why: exit status 1 and the refusal.
fn assert_refused(run: &Output, prefix: &Path) {
    assert_eq!(run.status.code(), Some(1), "{}", transcript(run));
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(
        said.contains(&format!("refusing to install into {}", prefix.display())),
        "{}",
        transcript(run)
    );
}

/// (a) `~/.local` holds the user's own presets in `share/crossfx/presets`, where `cp -a` would
/// lay the factory presets over every one of the same name: refused, and the user's file and
/// everything else under the prefix stay as they were.
#[test]
#[ignore = "crossfx-rename: until integration"]
fn install_sh_refuses_a_prefix_in_the_users_home_and_leaves_their_presets_alone() {
    let installer = Installer::new("install-home");
    let home = installer.dir("home");
    let prefix = home.join(".local");
    let mine = prefix.join("share/crossfx/presets/Input/Mine.toml");
    fs::create_dir_all(mine.parent().expect("a folder")).expect("the presets' folder is made");
    fs::write(&mine, "the user's own voice preset\n").expect("the preset is written");
    let before = snapshot(&prefix);

    let shell = Shell {
        home: &home,
        data_home: None,
        sudo_user: None,
        stand_ins: None,
    };
    let run = run_install(&installer.script(), &prefix, &shell);

    assert_refused(&run, &prefix);
    assert_eq!(text(&mine), "the user's own voice preset\n");
    assert_eq!(snapshot(&prefix), before, "the refused prefix was changed");
}

/// (b) Under `sudo`, `$HOME` is root's: the home that counts is the invoking user's, which
/// `getent passwd "$SUDO_USER"` names.
#[test]
#[ignore = "crossfx-rename: until integration"]
fn install_sh_under_sudo_refuses_the_home_getent_names_for_the_invoking_user() {
    let installer = Installer::new("install-sudo");
    let home = installer.dir("someone");
    let root_home = installer.dir("root");
    let prefix = home.join(".local");
    fs::create_dir_all(&prefix).expect("the prefix is made");
    let before = snapshot(&prefix);
    let stand_ins = installer.dir("stand-ins");
    write_executable(
        &stand_ins.join("getent"),
        &format!(
            "#!/bin/sh\n[ \"$1\" = passwd ] && [ \"$2\" = someone ] || exit 2\n\
             echo 'someone:x:1000:1000:Someone:{}:/bin/sh'\n",
            home.display()
        ),
    );

    let shell = Shell {
        home: &root_home,
        data_home: None,
        sudo_user: Some("someone"),
        stand_ins: Some(&stand_ins),
    };
    let run = run_install(&installer.script(), &prefix, &shell);

    assert_refused(&run, &prefix);
    assert_eq!(snapshot(&prefix), before, "the refused prefix was changed");
}

/// (c) A prefix outside the home whose `share` is the user's data home, `$XDG_DATA_HOME`.
#[test]
#[ignore = "crossfx-rename: until integration"]
fn install_sh_refuses_a_prefix_whose_share_is_the_users_data_home() {
    let installer = Installer::new("install-data-home");
    let home = installer.dir("home");
    let elsewhere = installer.dir("elsewhere");
    let data_home = elsewhere.join("share");
    fs::create_dir_all(&data_home).expect("the data home is made");
    let before = snapshot(&elsewhere);

    let shell = Shell {
        home: &home,
        data_home: Some(&data_home),
        sudo_user: None,
        stand_ins: None,
    };
    let run = run_install(&installer.script(), &elsewhere, &shell);

    assert_refused(&run, &elsewhere);
    assert_eq!(
        snapshot(&elsewhere),
        before,
        "the refused prefix was changed"
    );
}

/// (d) A prefix outside the home: installed under CrossFX's names, with the user unit and the
/// D-Bus activation file starting the binary where it now is.
#[test]
#[ignore = "crossfx-rename: until integration"]
fn install_sh_installs_into_a_prefix_outside_the_home_and_starts_crossfx_from_there() {
    let installer = Installer::new("install-prefix");
    let home = installer.dir("home");
    let prefix = installer.dir("prefix");

    let shell = Shell {
        home: &home,
        data_home: None,
        sudo_user: None,
        stand_ins: None,
    };
    let run = run_install(&installer.script(), &prefix, &shell);

    assert!(run.status.success(), "{}", transcript(&run));
    assert!(prefix.join("bin/crossfx").is_file(), "no bin/crossfx");
    for presets in ["Factsoft", "BonusPresets", "Input"] {
        let dir = prefix.join("share/crossfx/presets").join(presets);
        assert!(dir.is_dir(), "no {}", dir.display());
    }
    let binary = format!("{}/bin/crossfx", prefix.display());
    let unit = text(prefix.join("lib/systemd/user/crossfx.service"));
    assert!(
        unit.lines()
            .any(|line| line.trim() == format!("ExecStart={binary} --activated")),
        "the unit does not start {binary}:\n{unit}"
    );
    let activation =
        text(prefix.join("share/dbus-1/services/io.github.blackixxce12.CrossFX.service"));
    assert!(
        activation
            .lines()
            .any(|line| line.trim() == format!("Exec={binary} --activated")),
        "the activation file does not start {binary}:\n{activation}"
    );
}
