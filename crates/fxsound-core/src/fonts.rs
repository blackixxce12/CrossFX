//! Where FxSound looks for the Gilroy the user licensed, and where it keeps the files they add.
//!
//! FxSound neither ships nor downloads Gilroy ([`crate::InterfaceFont`]). The files the user adds
//! in Settings ▸ General are copied into FxSound's own data folder
//! ([`crate::Settings::user_font_dir`]) and read from there at every start; a Gilroy installed on
//! the system is found in the system's font folders ([`system_font_dirs`]) by the files' names,
//! and only those files are read. Whether a file really is a Gilroy, and which of its faces, is
//! the name table's to say (`fxsound_ui::typeface::read_face`): this module only finds and keeps
//! the bytes.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The largest font file read. Gilroy's faces are tens of kilobytes, and a whole CJK face is a
/// few megabytes; a file past this is not a face the window would be set in.
pub const MAX_FONT_FILE: u64 = 16 * 1024 * 1024;

/// How deep under a font folder files are looked for: `/usr/share/fonts/opentype/gilroy/` is
/// two folders down, and a family unpacked into `~/.local/share/fonts` keeps its archive's own.
const MAX_DEPTH: usize = 4;

/// The system's font folders, in the order they are searched: the user's own before the
/// system-wide ones, as a font installed for one user is meant to win.
///
/// - Linux: `$XDG_DATA_HOME/fonts` (`~/.local/share/fonts`), `~/.fonts`, `/usr/local/share/fonts`
///   and `/usr/share/fonts`, fontconfig's own defaults.
/// - Windows: `%LOCALAPPDATA%\Microsoft\Windows\Fonts`, where "Install" puts a font for one user,
///   and `%WINDIR%\Fonts`, where "Install for all users" does.
#[must_use]
pub fn system_font_dirs() -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    #[cfg(target_os = "linux")]
    {
        if let Some(data) = dirs::data_dir() {
            found.push(data.join("fonts"));
        }
        if let Some(home) = dirs::home_dir() {
            found.push(home.join(".local").join("share").join("fonts"));
            found.push(home.join(".fonts"));
        }
        found.push(PathBuf::from("/usr/local/share/fonts"));
        found.push(PathBuf::from("/usr/share/fonts"));
    }
    #[cfg(windows)]
    {
        if let Some(local) = dirs::data_local_dir() {
            found.push(local.join("Microsoft").join("Windows").join("Fonts"));
        }
        if let Some(windows) = std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot"))
        {
            found.push(PathBuf::from(windows).join("Fonts"));
        }
    }
    // `$XDG_DATA_HOME` is `~/.local/share` unless the user moved it.
    let mut unique = Vec::with_capacity(found.len());
    for dir in found {
        if !unique.contains(&dir) {
            unique.push(dir);
        }
    }
    unique
}

/// A font file found, read whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontFile {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
}

/// Whether `path` names a font file the window can be set in: `.ttf` or `.otf`, in any case. A
/// collection (`.ttc`) is not one; Gilroy is sold as single faces.
#[must_use]
pub fn is_font_file(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|ext| ext.eq_ignore_ascii_case("ttf") || ext.eq_ignore_ascii_case("otf"))
}

/// The font files under `dir`, to [`MAX_DEPTH`] folders down, whose file name contains `word` in
/// any case — every one for an empty `word` — each read whole, in the order of their paths.
///
/// A folder that is not there is no files. A file that cannot be read, or is larger than
/// [`MAX_FONT_FILE`], is passed over and the log says why; nothing found is not an error.
#[must_use]
pub fn font_files(dir: &Path, word: &str) -> Vec<FontFile> {
    let word = word.to_lowercase();
    let mut paths = Vec::new();
    collect(dir, &word, MAX_DEPTH, &mut paths);
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| match read_font_file(&path) {
            Ok(bytes) => Some(FontFile { path, bytes }),
            Err(error) => {
                log::info!("{}: {error}; not read", path.display());
                None
            }
        })
        .collect()
}

fn collect(dir: &Path, word: &str, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if depth > 0 {
                collect(&path, word, depth - 1, out);
            }
        } else if is_font_file(&path)
            && path
                .file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| name.to_lowercase().contains(word))
        {
            out.push(path);
        }
    }
}

/// `path` read whole, if it is a file no larger than [`MAX_FONT_FILE`].
///
/// # Errors
///
/// The file's own error, or [`std::io::ErrorKind::FileTooLarge`] for one past the limit.
pub fn read_font_file(path: &Path) -> std::io::Result<Vec<u8>> {
    let size = std::fs::metadata(path)?.len();
    if size > MAX_FONT_FILE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::FileTooLarge,
            format!("{size} bytes, more than a face"),
        ));
    }
    std::fs::read(path)
}

/// Keep a font file the user added: `bytes` written into `dir` under `name`, durably, replacing a
/// file of that name ([`crate::atomic::write`]), the folder made if it is not there. Returns
/// where it went.
///
/// A name that is not a font file's ([`is_font_file`]) gets one's extension after it: the file
/// picker lets a name be typed in, so a face can come without `.ttf` or `.otf`, and the folder is
/// read back through [`font_files`], which takes no other. Which one, the bytes say: `.otf` for
/// an OpenType face with CFF outlines (`OTTO`), `.ttf` for the TrueType ones (`0x00010000`,
/// `true`). Added rather than put in place of what the name ends with, so that two faces named
/// apart stay apart.
///
/// # Errors
///
/// A `name` that is not a plain file name (`..`, a path), and whatever writing the file meets.
pub fn keep(dir: &Path, name: &OsStr, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let plain = Path::new(name)
        .file_name()
        .filter(|plain| *plain == name)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{} is not a file name", Path::new(name).display()),
            )
        })?;
    let mut file_name = plain.to_owned();
    if !is_font_file(Path::new(plain)) {
        file_name.push(if bytes.starts_with(b"OTTO") {
            ".otf"
        } else {
            ".ttf"
        });
    }
    let path = dir.join(file_name);
    crate::atomic::write(&path, bytes)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_found_by_their_name_at_any_depth_and_read_whole() {
        let root = tempfile::tempdir().expect("a scratch folder");
        let nested = root.path().join("opentype").join("gilroy");
        std::fs::create_dir_all(&nested).expect("folders");
        std::fs::write(nested.join("Gilroy-Bold.OTF"), b"bold").expect("written");
        std::fs::write(root.path().join("gilroy-regular.ttf"), b"regular").expect("written");
        std::fs::write(root.path().join("Manrope-Regular.ttf"), b"manrope").expect("written");
        std::fs::write(root.path().join("Gilroy-Regular.woff2"), b"web font").expect("written");
        std::fs::write(root.path().join("Gilroy.ttc"), b"collection").expect("written");

        let found = font_files(root.path(), "Gilroy");
        let names: Vec<String> = found
            .iter()
            .map(|file| {
                file.path
                    .file_name()
                    .expect("a name")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(
            names.contains(&"gilroy-regular.ttf".to_owned()),
            "{names:?}"
        );
        assert!(names.contains(&"Gilroy-Bold.OTF".to_owned()), "{names:?}");
        let bold = found
            .iter()
            .find(|file| file.path.ends_with("Gilroy-Bold.OTF"))
            .expect("found");
        assert_eq!(bold.bytes, b"bold");
        // An empty word takes every font file: FxSound's own folder is read whole.
        assert_eq!(font_files(root.path(), "").len(), 3);
        // A folder that is not there holds nothing.
        assert!(font_files(&root.path().join("nowhere"), "gilroy").is_empty());
    }

    #[test]
    fn a_kept_file_lands_under_its_own_name_and_replaces_one_of_that_name() {
        let root = tempfile::tempdir().expect("a scratch folder");
        let dir = root.path().join("fxsound").join("fonts");
        let kept = keep(&dir, OsStr::new("Gilroy-Regular.ttf"), b"first").expect("kept");
        assert_eq!(kept, dir.join("Gilroy-Regular.ttf"));
        keep(&dir, OsStr::new("Gilroy-Regular.ttf"), b"second").expect("kept again");
        assert_eq!(std::fs::read(&kept).expect("read"), b"second");
        // Nothing but a plain name: a path would put the file somewhere else.
        for name in ["../Gilroy.ttf", "..", "sub/Gilroy.ttf"] {
            assert!(keep(&dir, OsStr::new(name), b"x").is_err(), "{name}");
        }
        assert!(!root.path().join("fxsound").join("Gilroy.ttf").exists());
    }

    #[test]
    fn a_face_picked_without_a_font_files_name_is_kept_under_one_and_read_back() {
        // The picker takes a typed name as it is: a face saved without `.ttf` or `.otf`, or with
        // another ending, was copied and then never read, as the folder is read through
        // `font_files`, which takes nothing else.
        let root = tempfile::tempdir().expect("a scratch folder");
        let dir = root.path().join("fonts");
        let truetype = [&[0, 1, 0, 0][..], b"glyf and the rest"].concat();
        let cff = [&b"OTTO"[..], b"CFF and the rest"].concat();
        let apple = [&b"true"[..], b"glyf and the rest"].concat();
        let cases: [(&str, &[u8], &str); 5] = [
            ("Gilroy-Regular", &truetype, "Gilroy-Regular.ttf"),
            ("Gilroy-Bold", &cff, "Gilroy-Bold.otf"),
            ("Gilroy-SemiBold.bin", &apple, "Gilroy-SemiBold.bin.ttf"),
            // A dot in the name is not an extension to put the right one in place of.
            ("Gilroy.Medium", &truetype, "Gilroy.Medium.ttf"),
            // A font file's name stays as it is.
            ("gilroy-light.OTF", &cff, "gilroy-light.OTF"),
        ];
        for (name, bytes, kept_as) in cases {
            let kept = keep(&dir, OsStr::new(name), bytes).expect("kept");
            assert_eq!(kept, dir.join(kept_as), "{name}");
        }
        let read: Vec<(String, Vec<u8>)> = font_files(&dir, "")
            .into_iter()
            .map(|file| {
                let name = file.path.file_name().expect("a name");
                (name.to_string_lossy().into_owned(), file.bytes)
            })
            .collect();
        assert_eq!(read.len(), cases.len(), "{read:?}");
        for (name, bytes, kept_as) in cases {
            assert!(
                read.iter()
                    .any(|(file, read)| file == kept_as && read.as_slice() == bytes),
                "{name} is not read back as {kept_as}"
            );
        }
    }

    #[test]
    fn a_file_too_large_for_a_face_is_not_read() {
        let root = tempfile::tempdir().expect("a scratch folder");
        let path = root.path().join("Gilroy-Huge.ttf");
        let file = std::fs::File::create(&path).expect("created");
        file.set_len(MAX_FONT_FILE + 1).expect("sized");
        let error = read_font_file(&path).expect_err("too large");
        assert_eq!(error.kind(), std::io::ErrorKind::FileTooLarge);
        assert!(font_files(root.path(), "gilroy").is_empty());
    }

    #[test]
    fn the_system_folders_are_the_platforms_own() {
        let dirs = system_font_dirs();
        if cfg!(target_os = "linux") {
            assert!(
                dirs.contains(&PathBuf::from("/usr/share/fonts")),
                "{dirs:?}"
            );
            assert!(
                dirs.contains(&PathBuf::from("/usr/local/share/fonts")),
                "{dirs:?}"
            );
            assert!(dirs.iter().any(|dir| dir.ends_with(".fonts")), "{dirs:?}");
        }
        if cfg!(windows) {
            assert!(
                dirs.iter()
                    .any(|dir| dir.ends_with(Path::new("Microsoft/Windows/Fonts"))),
                "{dirs:?}"
            );
            assert!(dirs.iter().any(|dir| dir.ends_with("Fonts")), "{dirs:?}");
        }
        for (i, dir) in dirs.iter().enumerate() {
            assert!(!dirs[i + 1..].contains(dir), "{dir:?} twice");
        }
    }
}
