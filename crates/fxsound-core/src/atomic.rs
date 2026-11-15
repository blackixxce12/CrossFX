//! Replacing a file without ever leaving a half-written one behind.
//!
//! Every persistent file this application owns — the settings, a saved preset, the autosave of
//! unsaved edits — is rewritten in place while the application is running, and two of those are
//! written on paths a user triggers constantly: the autosave fires on every preset switch with
//! unsaved changes and again on shutdown. A plain `fs::write` truncates first, so an interrupted
//! one leaves a zero-length or half-written file, and the reader on the next launch finds a
//! preset it cannot parse where a perfectly good one used to be.
//!
//! The sequence here is the usual durable-replace: write a temporary file beside the target,
//! `fsync` it, rename over the target, then `fsync` the directory so the rename itself is on
//! disk. The temp file lives in the same directory as the target because a rename is only atomic
//! within one filesystem, and it carries the process id so two instances cannot collide on it.
//!
//! What this does **not** promise: that the *new* contents survive a power cut. A durable replace
//! guarantees the reader sees either the old file or the new one, never a mixture — which is the
//! property that matters, because the alternative is losing data that was already safely on disk.
//!
//! A file that is a symbolic link stays one. Dotfile managers — GNU Stow, chezmoi's symlink mode —
//! keep `settings.toml` as a link into a repository, and a rename over the link itself would turn
//! it into a plain file: the repository's copy would stop receiving changes without a word. So
//! the rename happens where the link points, beside the file it names.
//!
//! A link to a read-only file is not written through, and neither is it replaced: the write
//! fails. home-manager's default links end at such a file in the Nix store. Replacing the link
//! would leave a plain file in the way of the next `home-manager switch`, and renaming over the
//! file would change the store itself on a single-user Nix install, whose user owns
//! `/nix/store`; where the store is read-only or root's, the rename could not happen anyway. So
//! every save through such a link fails; home-manager's `mkOutOfStoreSymlink` makes a link to a
//! file the user can write, and that one is saved through as any other.
//!
//! The same module moves a file out of the way without replacing anything already there
//! ([`rename_to_free_name`]): a settings file that does not load, a deleted preset the trash cannot
//! take.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Distinguishes temporary files written by the same process in the same millisecond.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Replace `path` with `contents`, atomically and durably.
///
/// Creates the parent directory if it is missing. On any failure the temporary file is removed
/// and the existing file is left exactly as it was.
///
/// A `path` that is a symbolic link, or a chain of them, is followed to the file it names, and
/// that file is the one replaced: the link is left as it is. A link that points at nothing yet
/// gets its file made where it points. A link to a read-only file — no write permission for
/// anyone, as on every file in the Nix store — fails with [`io::ErrorKind::PermissionDenied`],
/// and nothing is written anywhere.
pub fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let target = follow_links(path);
    if target.as_path() != path {
        refuse_read_only(&target)?;
    }
    let path = target.as_path();
    let parent = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    std::fs::create_dir_all(parent)?;

    let stem = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let temp = parent.join(format!(
        ".{stem}.{}.{}.tmp",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));

    match write_and_sync(&temp, contents).and_then(|()| std::fs::rename(&temp, path)) {
        Ok(()) => {}
        Err(err) => {
            let _ = std::fs::remove_file(&temp);
            return Err(err);
        }
    }

    // Without this the rename can still be lost on a crash even though the file's contents were
    // synced: the directory entry is its own piece of metadata. A filesystem that refuses to open
    // a directory for this is not a reason to report the write as failed — the data is already
    // there.
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// As [`write()`], but first copies any existing file to `<path>.bak`.
///
/// For files a user authored and cannot regenerate. The copy is not itself atomic — it is a
/// convenience for someone who overwrote the wrong preset, not a guarantee.
pub fn write_with_backup(path: &Path, contents: &[u8]) -> io::Result<()> {
    if path.exists() {
        let mut backup = path.as_os_str().to_owned();
        backup.push(".bak");
        if let Err(err) = std::fs::copy(path, Path::new(&backup)) {
            log::warn!("could not back up {}: {err}", path.display());
        }
    }
    write(path, contents)
}

fn write_and_sync(temp: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write as _;

    let mut file = std::fs::File::create(temp)?;
    file.write_all(contents)?;
    file.sync_all()
}

/// The file at the end of a link, when it is one nobody may write: see the module's notes on
/// home-manager. A rename only needs the directory to be writable, so without this a single-user
/// Nix install would have its store changed in place.
fn refuse_read_only(target: &Path) -> io::Result<()> {
    match std::fs::metadata(target) {
        Ok(meta) if meta.is_file() && meta.permissions().readonly() => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} is read-only; the link to it is not written through",
                target.display()
            ),
        )),
        _ => Ok(()),
    }
}

/// Linux's own limit on the links one lookup follows (`MAXSYMLINKS`).
const MAX_LINKS: usize = 40;

/// The file `path` names once the symbolic links in its last component are followed: `path`
/// itself when it is not a link. A relative link is read against the directory it sits in, as
/// the kernel reads it. Links that go round in a loop, or deeper than the kernel would follow,
/// give `path` back, and the write then replaces the link as a plain rename would: there is no
/// file at the end of them to keep.
fn follow_links(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    for _ in 0..MAX_LINKS {
        let is_link =
            std::fs::symlink_metadata(&current).is_ok_and(|meta| meta.file_type().is_symlink());
        if !is_link {
            return current;
        }
        let Ok(target) = std::fs::read_link(&current) else {
            return current;
        };
        // `join` with an absolute target is the target.
        current = current
            .parent()
            .map_or_else(|| target.clone(), |dir| dir.join(&target));
    }
    path.to_path_buf()
}

/// Rename `path` to the first of `names` that nothing has yet, and return that name; whatever
/// already has one of them is left as it is.
///
/// The rename cannot land on a file that appears under the name in the meantime either: the new
/// name is made as a hard link, which fails rather than replace anything, and the old one then
/// dropped. A filesystem without hard links (FAT, exFAT), or a file this user may not link — one
/// someone else owns, under `fs.protected_hardlinks` — gets a check for the name and then a plain
/// rename. A `path` that is a symbolic link is moved as the link.
///
/// # Errors
/// The file could not be renamed, or every name was taken; it is where it was.
pub fn rename_to_free_name(
    path: &Path,
    names: impl IntoIterator<Item = PathBuf>,
) -> io::Result<PathBuf> {
    for candidate in names {
        match std::fs::hard_link(path, &candidate) {
            Ok(()) => {
                if let Err(err) = std::fs::remove_file(path) {
                    let _ = std::fs::remove_file(&candidate);
                    return Err(err);
                }
                return Ok(candidate);
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(_) if candidate.symlink_metadata().is_ok() => {}
            Err(_) => {
                std::fs::rename(path, &candidate)?;
                return Ok(candidate);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free name to move it to",
    ))
}

/// How many names [`aside_names`] offers before giving up.
const MAX_ASIDE: u32 = 1000;

/// `first`, then the same name numbered from 2 up — `settings.toml.bad`, `settings.toml.2.bad`,
/// `settings.toml.3.bad` — where a file that did not load is moved aside with
/// [`rename_to_free_name`]. The first such file is the one that held the user's own data, and a
/// later one that fails to load too must not take its place.
pub fn aside_names(first: PathBuf) -> impl Iterator<Item = PathBuf> {
    let name = first.file_name().map(ToOwned::to_owned).unwrap_or_default();
    let numbered_from = first.clone();
    std::iter::once(first)
        .chain((2..=MAX_ASIDE).map(move |n| numbered_from.with_file_name(numbered(&name, n))))
}

/// The first of [`aside_names`] that nothing has yet: where a file that does not load would be
/// moved now, for a report that moves nothing. `first` when all of them are taken.
#[must_use]
pub fn next_aside_name(first: PathBuf) -> PathBuf {
    aside_names(first.clone())
        .find(|candidate| candidate.symlink_metadata().is_err())
        .unwrap_or(first)
}

/// `Rock.fac` as the `n`th of its name: `Rock.n.fac`, the extension kept so that a file manager
/// still knows what it is. A name with no extension gets the number at its end.
#[must_use]
pub fn numbered(name: &OsStr, n: u32) -> OsString {
    let path = Path::new(name);
    match (path.file_stem(), path.extension()) {
        (Some(stem), Some(extension)) => {
            let mut out = stem.to_owned();
            out.push(format!(".{n}."));
            out.push(extension);
            out
        }
        _ => {
            let mut out = name.to_owned();
            out.push(format!(".{n}"));
            out
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A new directory of the test's own, removed with the handle — also when the test panics.
    fn temp_dir(name: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!("crossfx-atomic-{name}-"))
            .tempdir()
            .expect("create the test directory")
    }

    #[test]
    fn a_write_creates_the_file_and_its_parent() {
        let dir = temp_dir("create");
        let path = dir.path().join("nested").join("settings.toml");
        write(&path, b"hello").expect("write");
        assert_eq!(std::fs::read(&path).expect("read"), b"hello");
    }

    #[test]
    fn a_write_leaves_no_temporary_file_behind() {
        let dir = temp_dir("clean");
        let path = dir.path().join("preset.fac");
        write(&path, b"one").expect("write");
        write(&path, b"two").expect("overwrite");
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .expect("list")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, vec!["preset.fac".to_owned()], "stray files: {left:?}");
        assert_eq!(std::fs::read(&path).expect("read"), b"two");
    }

    #[test]
    fn a_failed_write_leaves_the_previous_contents_intact() {
        let dir = temp_dir("failure");
        let path = dir.path().join("preset.fac");
        write(&path, b"original").expect("write");

        // A directory cannot be replaced by a file, so the rename fails while the temporary file
        // has already been written — the one ordering where a naive implementation would have
        // truncated the target first.
        let blocked = dir.path().join("blocked");
        std::fs::create_dir_all(&blocked).expect("create");
        assert!(write(&blocked, b"nope").is_err());

        assert_eq!(std::fs::read(&path).expect("read"), b"original");
        assert!(blocked.is_dir(), "the directory should be untouched");
    }

    #[test]
    fn a_backup_keeps_what_was_overwritten() {
        let dir = temp_dir("backup");
        let path = dir.path().join("My Preset.fac");
        write(&path, b"first").expect("write");
        write_with_backup(&path, b"second").expect("overwrite");

        assert_eq!(std::fs::read(&path).expect("read"), b"second");
        assert_eq!(
            std::fs::read(dir.path().join("My Preset.fac.bak")).expect("read the backup"),
            b"first"
        );
    }

    /// Every name in `dir`, sorted.
    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .expect("list")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// A symbolic link at `link` to `target`, a file or nothing yet, or `None` where this test
    /// process may not make one: Windows makes symbolic links only for an administrator or with
    /// Developer Mode on, and a test without one has nothing to check. Unix always may.
    ///
    /// # Panics
    /// The link could not be made for another reason.
    pub(crate) fn symlink(target: &Path, link: &Path) -> Option<()> {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).expect("a symbolic link");
        #[cfg(windows)]
        {
            /// `ERROR_PRIVILEGE_NOT_HELD`: no `SeCreateSymbolicLinkPrivilege`.
            const PRIVILEGE_NOT_HELD: i32 = 1314;
            match std::os::windows::fs::symlink_file(target, link) {
                Ok(()) => {}
                Err(err) if err.raw_os_error() == Some(PRIVILEGE_NOT_HELD) => {
                    eprintln!("skipped: this process may not make symbolic links ({err})");
                    return None;
                }
                Err(err) => panic!("a symbolic link at {}: {err}", link.display()),
            }
        }
        Some(())
    }

    /// Take away this user's right to read `path`'s contents, and nothing else: mode 000 on Unix;
    /// on Windows an entry denying everyone `FILE_READ_DATA`, made with `icacls` (no Win32 binding
    /// in a crate that forbids `unsafe`), which leaves the rename and the delete a move aside and
    /// the scratch directory's removal need. `false` when it could not be done.
    pub(crate) fn forbid_reading(path: &Path) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).is_ok()
        }
        #[cfg(windows)]
        {
            icacls(path, &["/deny", "*S-1-1-0:(RD)"])
        }
    }

    /// Give back what [`forbid_reading`] took: mode 600 on Unix, the denying entry removed on
    /// Windows.
    pub(crate) fn allow_reading(path: &Path) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).is_ok()
        }
        #[cfg(windows)]
        {
            icacls(path, &["/remove:d", "*S-1-1-0"])
        }
    }

    /// `icacls <path> <args>`, from the system directory; whether it succeeded. Everyone is named
    /// by its SID, `S-1-1-0`, which no translation of Windows renames.
    #[cfg(windows)]
    fn icacls(path: &Path, args: &[&str]) -> bool {
        let system = std::env::var_os("SystemRoot")
            .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        std::process::Command::new(system.join(r"System32\icacls.exe"))
            .arg(path)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    /// Windows: a rename over a file another handle holds open with `FILE_SHARE_DELETE` — as the
    /// standard library opens every file, and as a reader that holds the settings open does —
    /// replaces it as on Unix, and the reader keeps reading what it opened.
    #[cfg(windows)]
    #[test]
    fn a_write_replaces_a_file_a_reader_holds_open() {
        use std::io::Read as _;

        let dir = temp_dir("open");
        let path = dir.path().join("settings.toml");
        write(&path, b"old").expect("write");
        let mut reader = std::fs::File::open(&path).expect("open");

        write(&path, b"new").expect("a rename over an open file");

        assert_eq!(std::fs::read(&path).expect("read"), b"new");
        let mut held = Vec::new();
        reader.read_to_end(&mut held).expect("still readable");
        assert_eq!(held, b"old", "the reader keeps the file it opened");
        drop(reader);
        assert_eq!(listing(dir.path()), vec!["settings.toml".to_owned()]);
    }

    /// Windows: a file another program holds open without letting it be deleted — an editor, a
    /// backup or sync client — cannot be renamed over. The write fails, the file keeps what it
    /// had, and the temporary file is gone.
    #[cfg(windows)]
    #[test]
    fn a_write_over_a_file_held_without_share_delete_fails_and_leaves_it_whole() {
        use std::os::windows::fs::OpenOptionsExt as _;
        /// `FILE_SHARE_READ` alone: readers may come, no rename or delete may.
        const FILE_SHARE_READ: u32 = 1;

        let dir = temp_dir("locked");
        let path = dir.path().join("preset.fac");
        write(&path, b"original").expect("write");
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&path)
            .expect("open without FILE_SHARE_DELETE");

        assert!(write(&path, b"replacement").is_err());

        assert_eq!(std::fs::read(&path).expect("read"), b"original");
        assert_eq!(listing(dir.path()), vec!["preset.fac".to_owned()]);
        drop(held);
        write(&path, b"replacement").expect("once it is let go");
        assert_eq!(std::fs::read(&path).expect("read"), b"replacement");
    }

    /// A path past Windows' old `MAX_PATH` of 260 characters, which a preset of a long name in a
    /// deep profile reaches: the standard library hands such paths to Windows in their `\\?\`
    /// form, so the temporary file, the rename and the backup all work there as anywhere.
    #[test]
    fn a_write_and_its_backup_work_on_a_path_longer_than_260_characters() {
        let dir = temp_dir("long");
        let mut deep = dir.path().to_path_buf();
        for _ in 0..6 {
            deep.push("a directory with a name of some forty-odd chars");
        }
        let path = deep.join("A preset with a long name of its own, Rock Ballad.fac");
        assert!(path.as_os_str().len() > 300, "{}", path.display());

        write(&path, b"first").expect("write");
        write_with_backup(&path, b"second").expect("overwrite");

        assert_eq!(std::fs::read(&path).expect("read"), b"second");
        let mut backup = path.as_os_str().to_owned();
        backup.push(".bak");
        assert_eq!(std::fs::read(Path::new(&backup)).expect("backup"), b"first");
        assert_eq!(listing(&deep).len(), 2, "{:?}", listing(&deep));
    }

    #[test]
    fn a_write_through_a_symbolic_link_replaces_the_file_it_points_to_and_keeps_the_link() {
        // What GNU Stow and chezmoi's symlink mode leave in ~/.config: the file is a link into
        // the user's dotfiles. Renamed over, the link became a plain file and the dotfiles copy
        // silently stopped receiving changes.
        let dir = temp_dir("link");
        let dotfiles = dir.path().join("dotfiles");
        let config = dir.path().join("config");
        std::fs::create_dir_all(&dotfiles).expect("create");
        std::fs::create_dir_all(&config).expect("create");
        let real = dotfiles.join("settings.toml");
        std::fs::write(&real, b"power = false").expect("write");
        let link = config.join("settings.toml");
        let target = Path::new("..").join("dotfiles").join("settings.toml");
        let Some(()) = symlink(&target, &link) else {
            return;
        };

        write(&link, b"power = true").expect("write through the link");

        assert!(
            std::fs::symlink_metadata(&link)
                .expect("still there")
                .file_type()
                .is_symlink(),
            "the link is still a link"
        );
        assert_eq!(
            std::fs::read_link(&link).expect("read the link"),
            target,
            "pointing where it pointed"
        );
        assert_eq!(std::fs::read(&real).expect("read"), b"power = true");
        assert_eq!(listing(&config), vec!["settings.toml".to_owned()]);
        assert_eq!(listing(&dotfiles), vec!["settings.toml".to_owned()]);
    }

    #[test]
    fn a_chain_of_links_is_followed_to_the_file_at_its_end() {
        let dir = temp_dir("chain");
        let real = dir.path().join("real.toml");
        std::fs::write(&real, b"old").expect("write");
        let middle = dir.path().join("middle.toml");
        let Some(()) = symlink(&real, &middle) else {
            return;
        };
        let outer = dir.path().join("outer.toml");
        symlink(Path::new("middle.toml"), &outer).expect("a relative link");

        write(&outer, b"new").expect("write through both links");

        assert_eq!(std::fs::read(&real).expect("read"), b"new");
        for link in [&middle, &outer] {
            assert!(
                std::fs::symlink_metadata(link)
                    .expect("still there")
                    .file_type()
                    .is_symlink(),
                "{} is still a link",
                link.display()
            );
        }
    }

    #[test]
    fn a_link_that_points_at_nothing_yet_gets_its_file_made_where_it_points() {
        // A dotfiles checkout that has no settings yet: the first save fills it in.
        let dir = temp_dir("dangling");
        let link = dir.path().join("settings.toml");
        let real = dir.path().join("dotfiles").join("settings.toml");
        let Some(()) = symlink(&real, &link) else {
            return;
        };

        write(&link, b"first").expect("write through the link");

        assert_eq!(std::fs::read(&real).expect("read"), b"first");
        assert!(
            std::fs::symlink_metadata(&link)
                .expect("still there")
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn a_link_to_a_read_only_file_is_refused_and_leaves_the_link_and_the_file_as_they_were() {
        // What home-manager's default links end at: a read-only file in the Nix store. A
        // single-user Nix install lets its user write the store's directory, as this one is, so
        // without the refusal the rename would change the store in place.
        let dir = temp_dir("read-only");
        let store = dir.path().join("store");
        let config = dir.path().join("config");
        std::fs::create_dir_all(&store).expect("create");
        std::fs::create_dir_all(&config).expect("create");
        let real = store.join("hm_settings.toml");
        std::fs::write(&real, b"power = false").expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o444)).expect("chmod");
        }
        // Windows' read-only attribute, which is what `Permissions::readonly` reads there.
        #[cfg(windows)]
        {
            let mut read_only = std::fs::metadata(&real).expect("there").permissions();
            read_only.set_readonly(true);
            std::fs::set_permissions(&real, read_only).expect("make it read-only");
        }
        let link = config.join("settings.toml");
        let Some(()) = symlink(&real, &link) else {
            return;
        };

        let err = write(&link, b"power = true").expect_err("a read-only file is not written");

        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(std::fs::read(&real).expect("read"), b"power = false");
        assert!(
            std::fs::symlink_metadata(&link)
                .expect("still there")
                .file_type()
                .is_symlink(),
            "the link is still a link"
        );
        assert_eq!(listing(&config), vec!["settings.toml".to_owned()]);
        assert_eq!(listing(&store), vec!["hm_settings.toml".to_owned()]);
    }

    #[test]
    fn a_link_that_loops_is_replaced_as_a_plain_rename_would_replace_it() {
        // There is no file at the end of it to keep, and refusing would fail every save.
        let dir = temp_dir("loop");
        let a = dir.path().join("a.toml");
        let b = dir.path().join("b.toml");
        let Some(()) = symlink(&b, &a) else {
            return;
        };
        symlink(&a, &b).expect("a link");

        write(&a, b"plain").expect("write");

        assert_eq!(std::fs::read(&a).expect("read"), b"plain");
        assert!(std::fs::symlink_metadata(&a).expect("there").is_file());
    }

    #[test]
    fn moving_aside_takes_the_first_free_name_and_leaves_every_earlier_one_as_it_was() {
        let dir = temp_dir("aside");
        let path = dir.path().join("settings.toml");
        let first = dir.path().join("settings.toml.bad");
        std::fs::write(&first, b"the user's own settings").expect("write");
        std::fs::write(&path, b"a later broken file").expect("write");

        assert_eq!(
            next_aside_name(first.clone()),
            dir.path().join("settings.toml.2.bad"),
            "what a report would say"
        );
        let aside = rename_to_free_name(&path, aside_names(first.clone())).expect("move aside");

        assert_eq!(aside, dir.path().join("settings.toml.2.bad"));
        assert_eq!(std::fs::read(&aside).expect("read"), b"a later broken file");
        assert_eq!(
            std::fs::read(&first).expect("read"),
            b"the user's own settings",
            "the first one is never replaced"
        );
        assert!(!path.exists());
    }

    #[test]
    fn the_names_to_move_aside_to_start_plain_and_then_count_from_two() {
        let names: Vec<_> = aside_names(PathBuf::from("/c/apps.toml.bad"))
            .take(3)
            .collect();
        assert_eq!(
            names,
            [
                PathBuf::from("/c/apps.toml.bad"),
                PathBuf::from("/c/apps.toml.2.bad"),
                PathBuf::from("/c/apps.toml.3.bad"),
            ]
        );
        assert_eq!(numbered(OsStr::new("Rock.fac"), 3), "Rock.3.fac");
        assert_eq!(numbered(OsStr::new("Rock"), 2), "Rock.2");
    }

    #[test]
    fn a_backup_of_a_file_that_does_not_exist_yet_is_not_an_error() {
        let dir = temp_dir("backup-missing");
        let path = dir.path().join("new.fac");
        write_with_backup(&path, b"only").expect("write");
        assert_eq!(std::fs::read(&path).expect("read"), b"only");
        assert!(!dir.path().join("new.fac.bak").exists());
    }
}
