//! A file another program has open without sharing it.
//!
//! On Windows a program can open a file and deny everyone else reading it — a backup tool, an
//! antivirus scan, a sync client, an editor — and the read then fails with
//! `ERROR_SHARING_VIOLATION` (32), or with `ERROR_LOCK_VIOLATION` (33) for a locked byte range.
//! Both pass in a moment, and neither says anything about what the file holds. So the loaders
//! ([`crate::Settings::load_from`], [`crate::AppRules::load_from`]) wait a little for the holder
//! to let go ([`read_to_string`]), and when it does not, they neither move the file aside as
//! broken nor let a save replace it: the user's file is all there, and the defaults CrossFX runs
//! on meanwhile are not what the user chose. The path is remembered as [`left_unread`], every
//! save over it is refused ([`refuse_if_unread`]), and the next load that reads it lets saves
//! through again. Unix has no such locks; there nothing here ever applies.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// How many times [`read_to_string`] tries a held file before it gives up.
const TRIES: u32 = 5;

/// The wait between two of those tries: about half a second in all.
const PAUSE: Duration = Duration::from_millis(100);

/// The files a load found held and left unread, which no save may replace.
static UNREAD: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Whether `err` says another program has the file open without sharing it, or has locked a
/// part of it: an error that passes, not one about the file.
#[must_use]
pub fn is_held(err: &io::Error) -> bool {
    // ERROR_SHARING_VIOLATION and ERROR_LOCK_VIOLATION. The same numbers are EPIPE and EDOM on
    // Unix, which a read of a regular file never gives anyway.
    cfg!(windows) && matches!(err.raw_os_error(), Some(32 | 33))
}

/// [`std::fs::read_to_string`], tried again a few times while the file is [held](is_held).
///
/// # Errors
///
/// What the last read gave.
pub fn read_to_string(path: &Path) -> io::Result<String> {
    let mut tries = 1;
    loop {
        match std::fs::read_to_string(path) {
            Err(err) if is_held(&err) && tries < TRIES => {
                tries += 1;
                std::thread::sleep(PAUSE);
            }
            read => return read,
        }
    }
}

fn unread() -> std::sync::MutexGuard<'static, Vec<PathBuf>> {
    UNREAD.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Remember that the file at `path` is there but was held when it was to be loaded: until a
/// load reads it ([`read_after_all`]), [`refuse_if_unread`] refuses every save over it.
pub fn left_unread(path: &Path) {
    let mut unread = unread();
    if !unread.iter().any(|known| known == path) {
        unread.push(path.to_path_buf());
    }
}

/// A load read the file at `path`: saves may replace it again.
pub fn read_after_all(path: &Path) {
    unread().retain(|known| known != path);
}

/// Refuse a save over the file at `path` while it is [left unread](left_unread): what would be
/// written is the defaults and what changed since, not the user's file.
///
/// # Errors
///
/// [`io::ErrorKind::ResourceBusy`] while the file is left unread.
pub fn refuse_if_unread(path: &Path) -> io::Result<()> {
    if unread().iter().any(|known| known == path) {
        return Err(io::Error::new(
            io::ErrorKind::ResourceBusy,
            format!(
                "{} was in use by another program when it was loaded; it is not replaced \
                 before CrossFX has read it",
                path.display()
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_save_is_refused_only_between_a_held_load_and_a_read() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.toml");
        refuse_if_unread(&path).unwrap();
        left_unread(&path);
        left_unread(&path);
        let err = refuse_if_unread(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ResourceBusy);
        refuse_if_unread(&tmp.path().join("apps.toml")).unwrap();
        read_after_all(&path);
        refuse_if_unread(&path).unwrap();
    }

    #[test]
    fn errors_about_the_file_are_not_a_holder() {
        assert!(!is_held(&io::Error::from(io::ErrorKind::NotFound)));
        assert!(!is_held(&io::Error::from(io::ErrorKind::PermissionDenied)));
        // ERROR_ACCESS_DENIED, and EACCES on Unix.
        assert!(!is_held(&io::Error::from_raw_os_error(5)));
        assert_eq!(is_held(&io::Error::from_raw_os_error(32)), cfg!(windows));
        assert_eq!(is_held(&io::Error::from_raw_os_error(33)), cfg!(windows));
    }

    /// Open `path` as a backup tool or a sync client might: denying others every share but
    /// `share` (`FILE_SHARE_*`).
    #[cfg(windows)]
    fn hold(path: &Path, share: u32) -> std::fs::File {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(share)
            .open(path)
            .expect("hold the file")
    }

    /// `FILE_SHARE_DELETE`: the holder lets the file be renamed, but not read.
    #[cfg(windows)]
    const SHARE_DELETE: u32 = 4;

    #[cfg(windows)]
    #[test]
    fn held_settings_are_neither_moved_aside_nor_saved_over() {
        use crate::Settings;
        for share in [0, SHARE_DELETE] {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("settings.toml");
            let mine = b"power = false\n";
            std::fs::write(&path, mine).unwrap();
            assert!(Settings::default().power, "the file must differ from the defaults");

            let holder = hold(&path, share);
            let loaded = Settings::load_from(&path);
            assert_eq!(loaded, Settings::load_from(&tmp.path().join("missing.toml")));
            assert!(!Settings::bad_path(&path).exists(), "share {share}: moved aside");
            assert!(loaded.save_to(&path).is_err(), "share {share}: saved while held");
            drop(holder);
            // Let go of, the file is still not replaced by the defaults the load gave.
            let err = loaded.save_to(&path).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::ResourceBusy);
            assert_eq!(std::fs::read(&path).unwrap(), mine, "share {share}");

            // The next load reads the user's own values, and saves go through again.
            let again = Settings::load_from(&path);
            assert!(!again.power, "share {share}");
            again.save_to(&path).unwrap();
            assert!(!Settings::load_from(&path).power);
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_held_app_store_is_neither_moved_aside_nor_saved_over() {
        use crate::AppRules;
        for share in [0, SHARE_DELETE] {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("apps.toml");
            AppRules::default().save_to(&path).unwrap();
            let mine = std::fs::read(&path).unwrap();

            let holder = hold(&path, share);
            let loaded = AppRules::load_from(&path);
            assert!(!AppRules::bad_path(&path).exists(), "share {share}: moved aside");
            assert!(loaded.save_to(&path).is_err(), "share {share}: saved while held");
            drop(holder);
            assert!(loaded.save_to(&path).is_err(), "share {share}: saved before a read");
            assert_eq!(std::fs::read(&path).unwrap(), mine, "share {share}");

            AppRules::load_from(&path).save_to(&path).unwrap();
        }
    }
}
