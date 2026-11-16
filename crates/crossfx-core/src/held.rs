//! A file another program has open without sharing it.
//!
//! On Windows a program can open a file and deny everyone else reading it — a backup tool, an
//! antivirus scan, a sync client, an editor — and the read then fails with
//! `ERROR_SHARING_VIOLATION` (32), or with `ERROR_LOCK_VIOLATION` (33) for a locked byte range.
//! Both pass in a moment, and neither says anything about what the file holds. So the loaders
//! ([`crate::Settings::load_from`], [`crate::AppRules::load_from`]) wait a little for the holder
//! to let go ([`read_to_string`]), and when it does not, they neither move the file aside as
//! broken nor let a save replace it: the user's file is all there, and the defaults CrossFX runs
//! on meanwhile are not what the user chose. The path is remembered as [`left_unread`], with the
//! text of what the load gave instead.
//!
//! A save over such a file ([`merge_if_unread`]) reads it again. While it is still held, the save
//! is refused. Once it can be read, what changed since the load (or since the last such save) is
//! written into the user's file, and everything else in it stays as the user had it: the
//! session's changes are kept, and so is the file. The next load that reads the file ends all
//! this ([`read_after_all`]). Unix has no such locks; there nothing here ever applies.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// How many times [`read_to_string`] tries a held file before it gives up.
const TRIES: u32 = 5;

/// The wait between two of those tries: about half a second in all.
const PAUSE: Duration = Duration::from_millis(100);

/// The files a load found held and left unread, each with the TOML text of what this run last
/// wrote into it, or of what the load gave in its place: the base a save's changes are told
/// from.
static UNREAD: Mutex<Vec<(PathBuf, String)>> = Mutex::new(Vec::new());

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

fn unread() -> std::sync::MutexGuard<'static, Vec<(PathBuf, String)>> {
    UNREAD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Remember that the file at `path` is there but was held when it was to be loaded, and that
/// `loaded` (TOML) is what the load gave instead: until a load reads it ([`read_after_all`]),
/// every save over it goes through [`merge_if_unread`].
pub fn left_unread(path: &Path, loaded: &str) {
    let mut unread = unread();
    match unread.iter_mut().find(|(known, _)| known == path) {
        Some((_, base)) => loaded.clone_into(base),
        None => unread.push((path.to_path_buf(), loaded.to_owned())),
    }
}

/// A load read the file at `path`: saves replace it again.
pub fn read_after_all(path: &Path) {
    unread().retain(|(known, _)| known != path);
}

/// Whether the file at `path` is [left unread](left_unread): the running instance is on
/// defaults where the user's file may say otherwise.
#[must_use]
pub fn is_left_unread(path: &Path) -> bool {
    unread().iter().any(|(known, _)| known == path)
}

/// How the entries of one top-level array of a file are told apart, for [`merge`]: `key` names
/// the entry a TOML value is (`None` for one that names nothing, which a hand edit can leave),
/// and two entries with one name are the same entry, of which the file keeps one.
#[derive(Debug, Clone, Copy)]
pub struct Keyed {
    /// The array's key at the file's top level.
    pub array: &'static str,
    /// The name of the entry a value is.
    pub key: fn(&toml::Value) -> Option<String>,
}

/// What a save of `ours` (TOML) over the file at `path` writes. `None` when the file is not
/// [left unread](left_unread): `ours`, as it is. Otherwise the file is read again, and the
/// answer is the user's file with what changed in `ours` since the load, or since the save last
/// recorded with [`merged`], written into it ([`merge`], with `keyed` naming the arrays whose
/// entries are told apart by a key). Call [`merged`] with `ours` once that is written.
///
/// # Errors
///
/// [`io::ErrorKind::ResourceBusy`] while the file is still held; what the read gave when it
/// fails otherwise; [`io::ErrorKind::InvalidData`] when the file is not TOML (the next load sets
/// it aside, as it does any broken file).
pub fn merge_if_unread(path: &Path, ours: &str, keyed: &[Keyed]) -> io::Result<Option<String>> {
    let Some(base) = unread()
        .iter()
        .find(|(known, _)| known == path)
        .map(|(_, base)| base.clone())
    else {
        return Ok(None);
    };
    let theirs = match read_to_string(path) {
        Ok(theirs) => theirs,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            // Gone: nothing of the user's is left to keep.
            read_after_all(path);
            return Ok(None);
        }
        Err(err) if is_held(&err) => {
            return Err(io::Error::new(
                io::ErrorKind::ResourceBusy,
                format!(
                    "{} is in use by another program; it is not replaced before CrossFX has \
                     read it ({err})",
                    path.display()
                ),
            ));
        }
        Err(err) => return Err(err),
    };
    merge(&base, ours, &theirs, keyed).map(Some).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} is not a TOML file CrossFX can write its changes into",
                path.display()
            ),
        )
    })
}

/// `ours` (TOML) is now in the file at `path`, as far as this run's changes go: the next save's
/// changes are told from it.
pub fn merged(path: &Path, ours: &str) {
    if let Some((_, base)) = unread().iter_mut().find(|(known, _)| known == path) {
        ours.clone_into(base);
    }
}

/// `theirs` with what changed from `base` to `ours` written into it, as TOML; `None` when one of
/// the three is not a TOML document.
///
/// A key whose value `ours` changed takes that value, a key `ours` dropped goes, and every other
/// key keeps the value `theirs` gives it. Tables are merged key by key.
///
/// An array `keyed` names is a list of entries, each told apart by its key, and is merged entry
/// by entry, in the order `theirs` has them: an entry `ours` changed or added takes `ours`'s
/// value in its place, one `ours` dropped goes, and every other entry is `theirs`'s. An entry
/// only `ours` has goes in after the one before it in `ours` (first when there is none), and
/// when `ours` reordered the entries `base` had, those of `ours` take `ours`'s order in the
/// places they hold. So a ranked list such as the remembered devices keeps one entry per device,
/// and the user's ranking, beside what this session changed.
///
/// Any other array `ours` changed is `ours`'s elements followed by those only `theirs` has (in
/// neither `base` nor `ours`).
#[must_use]
pub fn merge(base: &str, ours: &str, theirs: &str, keyed: &[Keyed]) -> Option<String> {
    let base: toml::Table = toml::from_str(base).ok()?;
    let ours: toml::Table = toml::from_str(ours).ok()?;
    let mut theirs: toml::Table = toml::from_str(theirs).ok()?;
    merge_tables(&base, &ours, &mut theirs, keyed);
    toml::to_string_pretty(&theirs).ok()
}

fn merge_tables(base: &toml::Table, ours: &toml::Table, theirs: &mut toml::Table, keyed: &[Keyed]) {
    for key in base.keys() {
        if !ours.contains_key(key) {
            theirs.remove(key);
        }
    }
    for (key, mine) in ours {
        let was = base.get(key);
        if was == Some(mine) {
            continue;
        }
        let merged = match (was, mine, theirs.get_mut(key)) {
            (
                Some(toml::Value::Table(was)),
                toml::Value::Table(mine),
                Some(toml::Value::Table(other)),
            ) => {
                // Arrays are told apart by key only at the top level, where `keyed` names them.
                merge_tables(was, mine, other, &[]);
                continue;
            }
            (was, toml::Value::Array(mine), Some(toml::Value::Array(other))) => {
                let was = match was {
                    Some(toml::Value::Array(was)) => was.as_slice(),
                    _ => &[],
                };
                match keyed.iter().find(|keyed| keyed.array == key) {
                    Some(keyed) => toml::Value::Array(merge_entries(was, mine, other, keyed.key)),
                    None => {
                        let mut all = mine.clone();
                        all.extend(
                            other
                                .iter()
                                .filter(|value| !was.contains(value) && !mine.contains(value))
                                .cloned(),
                        );
                        toml::Value::Array(all)
                    }
                }
            }
            _ => mine.clone(),
        };
        theirs.insert(key.clone(), merged);
    }
}

/// A keyed array merged entry by entry: see [`merge`].
fn merge_entries(
    was: &[toml::Value],
    mine: &[toml::Value],
    other: &[toml::Value],
    key: fn(&toml::Value) -> Option<String>,
) -> Vec<toml::Value> {
    let find = |list: &'_ [toml::Value], name: &str| {
        list.iter()
            .find(|value| key(value).as_deref() == Some(name))
            .cloned()
    };
    let position = |list: &[toml::Value], name: &str| {
        list.iter()
            .position(|value| key(value).as_deref() == Some(name))
    };

    // The user's entries, in the user's order, each as this session left it.
    let mut out: Vec<toml::Value> = Vec::with_capacity(other.len() + mine.len());
    let mut named = std::collections::HashSet::new();
    for value in other {
        let Some(name) = key(value) else {
            out.push(value.clone());
            continue;
        };
        if !named.insert(name.clone()) {
            // A repeat only a hand edit makes: the first is the entry every lookup answers with.
            continue;
        }
        match (find(was, &name), find(mine, &name)) {
            (Some(_), None) => {}
            (before, Some(now)) if before.as_ref() != Some(&now) => out.push(now),
            _ => out.push(value.clone()),
        }
    }

    // The entries this session made that the file does not have, after the one before each.
    let mut after: Option<usize> = None;
    for value in mine {
        let Some(name) = key(value) else { continue };
        if let Some(at) = position(&out, &name) {
            after = Some(at);
            continue;
        }
        if named.contains(&name) || find(was, &name).as_ref() == Some(value) {
            // Dropped from the file by the user, and left as it was here: it stays dropped.
            continue;
        }
        let at = after.map_or(0, |at| at + 1);
        out.insert(at, value.clone());
        after = Some(at);
    }

    // A ranking this session changed: its entries take its order, in the places they hold.
    let shared = |list: &[toml::Value]| -> Vec<String> {
        list.iter()
            .filter_map(key)
            .filter(|name| position(was, name).is_some() && position(mine, name).is_some())
            .collect()
    };
    if shared(was) != shared(mine) {
        let order: Vec<String> = mine
            .iter()
            .filter_map(key)
            .filter(|name| position(&out, name).is_some())
            .collect();
        let places: Vec<usize> = order
            .iter()
            .filter_map(|name| position(&out, name))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let values: Vec<toml::Value> = order.iter().filter_map(|name| find(&out, name)).collect();
        for (place, value) in places.into_iter().zip(values) {
            out[place] = value;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_merged_into_only_between_a_held_load_and_a_read() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.toml");
        assert_eq!(merge_if_unread(&path, "a = 1\n", &[]).unwrap(), None);
        std::fs::write(&path, "a = 0\nb = 2\n").unwrap();
        left_unread(&path, "a = 0\n");
        left_unread(&path, "a = 0\n");
        assert!(is_left_unread(&path));
        assert!(!is_left_unread(&tmp.path().join("apps.toml")));
        let merged_text = merge_if_unread(&path, "a = 1\n", &[]).unwrap().unwrap();
        let table: toml::Table = toml::from_str(&merged_text).unwrap();
        assert_eq!(table["a"].as_integer(), Some(1));
        assert_eq!(table["b"].as_integer(), Some(2), "the file's own value");
        read_after_all(&path);
        assert!(!is_left_unread(&path));
        assert_eq!(merge_if_unread(&path, "a = 1\n", &[]).unwrap(), None);

        // A file that went away leaves nothing to keep.
        let gone = tmp.path().join("gone.toml");
        left_unread(&gone, "a = 0\n");
        assert_eq!(merge_if_unread(&gone, "a = 1\n", &[]).unwrap(), None);
        assert!(!is_left_unread(&gone));

        // One that is not TOML is not written over.
        let broken = tmp.path().join("broken.toml");
        std::fs::write(&broken, "a = = 1").unwrap();
        left_unread(&broken, "a = 0\n");
        let err = merge_if_unread(&broken, "a = 1\n", &[]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        read_after_all(&broken);
    }

    #[test]
    fn what_changed_is_written_into_the_users_file_and_nothing_else() {
        let base = "power = true\nname = \"a\"\ngone = 1\nlist = []\nkept = [1]\n\
                    [t]\nx = 1\ny = 2\n";
        let ours = "power = true\nname = \"b\"\nlist = [1]\nkept = [1]\n[t]\nx = 1\ny = 3\n";
        let theirs = "power = false\nname = \"a\"\ngone = 7\nlist = [2, 1]\nkept = [5]\n\
                      extra = 5\n[t]\nx = 9\ny = 2\n";
        let merged_text = merge(base, ours, theirs, &[]).unwrap();
        let table: toml::Table = toml::from_str(&merged_text).unwrap();
        let expected: toml::Table = toml::from_str(
            "power = false\nname = \"b\"\nlist = [1, 2]\nkept = [5]\nextra = 5\n\
             [t]\nx = 9\ny = 3\n",
        )
        .unwrap();
        assert_eq!(table, expected);
        assert_eq!(merge("a =", ours, theirs, &[]), None);
    }

    fn named(value: &toml::Value) -> Option<String> {
        value.get("id")?.as_str().map(str::to_owned)
    }

    const LIST: [Keyed; 1] = [Keyed {
        array: "list",
        key: named,
    }];

    /// The entries of `text`'s `list`, each as `id:value`.
    fn entries(text: &str) -> Vec<String> {
        let table: toml::Table = toml::from_str(text).unwrap();
        table["list"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                format!(
                    "{}:{}",
                    entry["id"].as_str().unwrap(),
                    entry.get("v").and_then(toml::Value::as_str).unwrap_or("")
                )
            })
            .collect()
    }

    fn list(items: &[(&str, &str)]) -> String {
        let mut text = String::from("list = [");
        for (id, v) in items {
            text.push_str(&format!("{{ id = \"{id}\", v = \"{v}\" }}, "));
        }
        text.push_str("]\n");
        text
    }

    #[test]
    fn a_keyed_list_keeps_one_entry_per_key_in_the_users_order() {
        // The review's case: a session on a held file remembers a preset for device X; the
        // user's file ranks A, X, B. X takes the session's preset in its place, once.
        let merged_text = merge(
            &list(&[]),
            &list(&[("X", "Music")]),
            &list(&[("A", "Rock"), ("X", "Gaming"), ("B", "")]),
            &LIST,
        )
        .unwrap();
        assert_eq!(entries(&merged_text), ["A:Rock", "X:Music", "B:"]);

        // A new entry goes in after the one before it in the session's list, or first.
        let merged_text = merge(
            &list(&[("A", "1")]),
            &list(&[("N", "0"), ("A", "1"), ("M", "2")]),
            &list(&[("C", "3"), ("A", "1"), ("B", "4")]),
            &LIST,
        )
        .unwrap();
        assert_eq!(entries(&merged_text), ["N:0", "C:3", "A:1", "M:2", "B:4"]);

        // One the session dropped goes; one it left alone keeps the user's value; one the user
        // dropped and the session left alone stays dropped; a hand edit's repeat goes.
        let merged_text = merge(
            &list(&[("A", "1"), ("B", "1"), ("C", "1")]),
            &list(&[("B", "1"), ("C", "1"), ("D", "1")]),
            &list(&[("A", "1"), ("B", "9"), ("B", "8"), ("E", "1")]),
            &LIST,
        )
        .unwrap();
        assert_eq!(entries(&merged_text), ["B:9", "D:1", "E:1"]);

        // A ranking the session changed is the session's, in the places its entries hold.
        let merged_text = merge(
            &list(&[("A", ""), ("B", "")]),
            &list(&[("B", ""), ("A", "")]),
            &list(&[("A", ""), ("C", ""), ("B", "")]),
            &LIST,
        )
        .unwrap();
        assert_eq!(entries(&merged_text), ["B:", "C:", "A:"]);

        // One the session did not reorder keeps the user's ranking.
        let merged_text = merge(
            &list(&[("A", "1"), ("B", "1")]),
            &list(&[("A", "2"), ("B", "1")]),
            &list(&[("B", "1"), ("A", "1")]),
            &LIST,
        )
        .unwrap();
        assert_eq!(entries(&merged_text), ["B:1", "A:2"]);
    }

    #[test]
    fn a_held_settings_file_keeps_one_entry_per_device() {
        use crate::{DeviceDirection, Settings};
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.toml");
        let mut users = Settings::default();
        for (device, preset) in [("A", "Rock"), ("X", "Gaming"), ("B", "")] {
            users.remember_device_preset(device, device, preset, "", DeviceDirection::Output);
        }
        users.save_to(&path).unwrap();
        let defaults = Settings::load_from(&tmp.path().join("missing.toml"));
        left_unread(&path, &toml::to_string_pretty(&defaults).unwrap());

        let mut session = defaults;
        session.remember_device_preset("X", "X", "Music", "", DeviceDirection::Output);
        session.save_to(&path).unwrap();
        let again = Settings::load_from(&path);
        let ranking: Vec<(&str, &str)> = again
            .device_configs
            .iter()
            .map(|entry| (entry.device_id.as_str(), entry.preset.as_str()))
            .collect();
        assert_eq!(ranking, [("A", "Rock"), ("X", "Music"), ("B", "")]);
        read_after_all(&path);
    }

    #[test]
    fn sanitise_keeps_the_first_entry_for_a_device() {
        let mut settings: crate::Settings = toml::from_str(
            "[[device_configs]]\ndevice_id = \"X\"\npreset = \"Music\"\n\
             [[device_configs]]\ndevice_id = \"A\"\n\
             [[device_configs]]\ndevice_id = \"X\"\npreset = \"Gaming\"\n\
             [[device_configs]]\ndevice_id = \"X\"\ndirection = \"input\"\n",
        )
        .unwrap();
        settings.sanitise();
        let left: Vec<(&str, &str)> = settings
            .device_configs
            .iter()
            .map(|entry| (entry.device_id.as_str(), entry.preset.as_str()))
            .collect();
        assert_eq!(left, [("X", "Music"), ("A", ""), ("X", "")]);
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
    fn held_settings_are_not_moved_aside_and_keep_the_users_values_and_the_sessions() {
        use crate::Settings;
        for share in [0, SHARE_DELETE] {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("settings.toml");
            let mine = b"power = false\n";
            std::fs::write(&path, mine).unwrap();
            assert!(
                Settings::default().power,
                "the file must differ from the defaults"
            );

            let holder = hold(&path, share);
            let mut loaded = Settings::load_from(&path);
            assert_eq!(
                loaded,
                Settings::load_from(&tmp.path().join("missing.toml"))
            );
            assert!(is_left_unread(&path));
            assert!(
                !Settings::bad_path(&path).exists(),
                "share {share}: moved aside"
            );
            loaded.hide_notifications = !loaded.hide_notifications;
            let err = loaded.save_to(&path).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::ResourceBusy, "share {share}");
            drop(holder);
            assert_eq!(std::fs::read(&path).unwrap(), mine, "share {share}");

            // Let go of, the file takes the session's change and keeps the user's own values.
            loaded.save_to(&path).unwrap();
            let again = Settings::load_from(&path);
            assert!(!again.power, "share {share}: the user's value");
            assert_eq!(
                again.hide_notifications, loaded.hide_notifications,
                "share {share}: the session's change"
            );
            assert!(!is_left_unread(&path));
            again.save_to(&path).unwrap();
            assert!(!Settings::load_from(&path).power);
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_held_app_store_is_neither_moved_aside_nor_saved_over_while_held() {
        use crate::AppRules;
        for share in [0, SHARE_DELETE] {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("apps.toml");
            AppRules::default().save_to(&path).unwrap();
            let mine = std::fs::read(&path).unwrap();

            let holder = hold(&path, share);
            let loaded = AppRules::load_from(&path);
            assert!(
                !AppRules::bad_path(&path).exists(),
                "share {share}: moved aside"
            );
            assert!(
                loaded.save_to(&path).is_err(),
                "share {share}: saved while held"
            );
            drop(holder);
            assert_eq!(std::fs::read(&path).unwrap(), mine, "share {share}");
            loaded.save_to(&path).unwrap();
            assert!(AppRules::load_from(&path).apps.is_empty(), "share {share}");
        }
    }
}
