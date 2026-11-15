//! Enumerating, selecting and persisting presets.
//!
//! Mirrors `FxController::initPresets()` / `setPreset()` / `autoSavePreset()`
//! (`fxsound/Source/GUI/FxController.cpp:841-876, 1048-1097`) with two deliberate differences,
//! both noted in `docs/spec/11-preset-format.md`:
//!
//! * the original lists presets in filesystem-glob order, which is effectively arbitrary; this
//!   port sorts deterministically — factory presets in their numeric order, the rest of the
//!   factory presets by name, then the user's by name;
//! * paths follow the XDG base directory specification instead of `%APPDATA%`.
//!
//! The store is generic over the file it holds. 0.4.0 has two kinds of preset — the `.fac` set
//! for the speakers and the TOML voice set for the microphone — and one discipline for listing
//! them: which copy wins when a name appears twice, what order the list is in, where an autosave
//! lives, when a `.bak` is kept. A rule that lives in two places is a rule that gets fixed in
//! one place; the shadowing bug this module carried in 0.3.0 would have been copied along with
//! it. [`PresetStore`] is the `.fac` store and [`crate::InputPresetStore`] the voice one.

use crate::trash::{self, Discarded};
use crate::{PresetError, new_preset_name, sanitise_preset_name};
use fxsound_core::{Preset, Settings};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

/// Where a preset came from, which decides whether it can be overwritten or deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PresetSource {
    /// Shipped with the application; read-only. `PresetType::AppPreset` in the original.
    Factory,
    /// Saved or imported by the user. `PresetType::UserPreset` in the original.
    User,
}

/// One entry in the preset list, matching `FxModel::Preset` (`FxModel.h:34-40`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetEntry {
    pub name: String,
    pub path: PathBuf,
    pub source: PresetSource,
    /// `true` when an autosave of its own exists, i.e. the user changed this preset without
    /// saving. Shown as a trailing `*` in the combo box.
    pub modified: bool,
}

/// A preset file format a [`Store`] can hold.
///
/// The store asks five things of a format: its extension, how to read and write one file, and
/// the name inside it — a preset is listed by the name it calls itself, never by its filename,
/// because the factory `.fac` set ships as `1.fac`..`12.fac` and calls itself General, Music,
/// Voice. The error constructors let the store report an unknown name, an unusable one and one
/// that would take another preset's file in the format's own error type rather than in a third
/// one every caller would have to convert.
pub trait PresetFile: Clone {
    /// The extension the store looks for and writes, without the dot.
    const EXTENSION: &'static str;
    /// What reading or writing one file can fail with.
    type Error: From<std::io::Error> + std::fmt::Display;

    fn load(path: &Path) -> Result<Self, Self::Error>;
    /// Replace `path` atomically.
    fn save(&self, path: &Path) -> Result<(), Self::Error>;
    /// As [`PresetFile::save`], keeping any previous file as `<path>.bak`.
    fn save_with_backup(&self, path: &Path) -> Result<(), Self::Error>;
    fn name(&self) -> &str;
    fn set_name(&mut self, name: &str);
    /// The error for a name the list does not hold.
    fn unknown(name: &str) -> Self::Error;
    /// The error for a name with nothing left in it once it has been made safe as a filename.
    fn empty_name() -> Self::Error;
    /// The error for a name whose file, `file`, is already the listed preset `existing`'s.
    fn shared_file(name: &str, existing: &str, file: &str) -> Self::Error;
    /// The preset as it is written for someone else to open: an export. The same preset unless
    /// the format has a reader elsewhere that expects something of it — the `.fac` the original
    /// FxSound for Windows reads puts a twenty-band curve back on the original's ladder (0.4.0
    /// audit R4), and its end bands as `end_bands` says.
    #[must_use]
    fn exported(&self, _end_bands: EndBands) -> Self {
        self.clone()
    }
}

/// Where an exported `.fac` puts its first and last band (0.4.0 audit R6).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EndBands {
    /// Back inside the range the original tunes them in, as 0.4.0 exports them: the default,
    /// and the only choice below «Like the original FxSound LLC» = Interface and sound
    /// ([`fxsound_core::eq::move_end_bands_back_inside_the_ladder`]).
    #[default]
    Shifted,
    /// Where they are, limited only to the equalizer's 10 Hz–21 kHz
    /// ([`fxsound_core::eq::keep_centres_inside_the_equalizer`]): the export window's choice from
    /// Interface and sound on, so the file plays on Windows as the curve plays here.
    AsTheyAre,
}

impl PresetFile for Preset {
    const EXTENSION: &'static str = "fac";
    type Error = PresetError;

    fn load(path: &Path) -> Result<Self, PresetError> {
        crate::load(path)
    }

    fn save(&self, path: &Path) -> Result<(), PresetError> {
        crate::save(self, path)
    }

    fn save_with_backup(&self, path: &Path) -> Result<(), PresetError> {
        crate::save_with_backup(self, path)
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn set_name(&mut self, name: &str) {
        self.name = name.to_owned();
    }

    fn unknown(name: &str) -> PresetError {
        PresetError::Unknown(name.to_owned())
    }

    fn empty_name() -> PresetError {
        PresetError::MissingName
    }

    fn shared_file(name: &str, existing: &str, file: &str) -> PresetError {
        PresetError::SharedFile {
            name: name.to_owned(),
            existing: existing.to_owned(),
            file: file.to_owned(),
        }
    }

    /// A twenty-band curve on the half-octave ladder is exported on the Windows one, band for
    /// band, which is what the original tunes twenty bands to; CrossFX moves it back when
    /// it reads it ([`fxsound_core::eq::move_off_the_windows_twenty_band_ladder`]).
    ///
    /// Then, [`EndBands::Shifted`], the first and last band go back inside the ladder's edges
    /// (0.4.0 audit R6): here they can be tuned half a band past them, where the original's
    /// wheels cannot follow, so a first band at 46 Hz is exported at 62.5 Hz. Nothing else moves,
    /// so a preset Windows made goes back out as it came. [`EndBands::AsTheyAre`] leaves them where
    /// they are, inside the equalizer's own 10 Hz–21 kHz. Importing takes a file as it is, wider
    /// centres and all.
    fn exported(&self, end_bands: EndBands) -> Self {
        let mut preset = self.clone();
        fxsound_core::eq::move_onto_the_windows_twenty_band_ladder(&mut preset.eq_bands);
        match end_bands {
            EndBands::Shifted => {
                fxsound_core::eq::move_end_bands_back_inside_the_ladder(&mut preset.eq_bands);
            }
            EndBands::AsTheyAre => {
                fxsound_core::eq::keep_centres_inside_the_equalizer(&mut preset.eq_bands);
            }
        }
        preset
    }
}

/// The full preset list plus the directories it was built from.
#[derive(Debug, Clone)]
pub struct Store<F: PresetFile> {
    entries: Vec<PresetEntry>,
    factory_dirs: Vec<PathBuf>,
    user_dir: PathBuf,
    autosave_dir: PathBuf,
    /// Where a deleted preset goes: `None` for the desktop's home trash, found when a preset is
    /// deleted ([`crate::trash`]); otherwise a directory laid out as a trash, for a store that must
    /// not touch the user's, such as a test's.
    trash_dir: Option<PathBuf>,
    format: PhantomData<F>,
}

/// The `.fac` store: the speakers' presets.
pub type PresetStore = Store<Preset>;

impl Store<Preset> {
    /// Build a store from the standard locations.
    ///
    /// Factory presets are looked for in the checkout the binary runs from, when it runs from one
    /// ([`crate::source_tree_of`]), and in the usual install prefixes, so the binary works both
    /// from `cargo run` and from a package; of the prefixes, the one the binary is installed under
    /// comes first ([`crate::install_data_dirs`]).
    #[must_use]
    pub fn with_default_dirs() -> Self {
        let factory_dirs = crate::shipped_preset_dirs(&["Factsoft", "BonusPresets"]);
        Self::with_dirs(factory_dirs, Settings::user_preset_dir()).with_home_trash()
    }
}

impl<F: PresetFile> Store<F> {
    /// Build a store from explicit directories. Used by the tests and by `--preset-dir`.
    ///
    /// Autosaves live in `AutoSave` under the user directory, whichever directory that is.
    ///
    /// A store built this way keeps what it deletes in a trash of its own, `.Trash` under the user
    /// directory, so that a test's deletions never reach the desktop's; the stores built from the
    /// standard locations use the desktop's ([`Store::with_home_trash`]).
    #[must_use]
    pub fn with_dirs(factory_dirs: Vec<PathBuf>, user_dir: PathBuf) -> Self {
        let autosave_dir = user_dir.join("AutoSave");
        let trash_dir = Some(user_dir.join(".Trash"));
        Self {
            entries: Vec::new(),
            factory_dirs,
            user_dir,
            autosave_dir,
            trash_dir,
            format: PhantomData,
        }
    }

    /// Send deleted presets to `dir`, laid out as a trash (`files/`, `info/`).
    #[must_use]
    pub fn with_trash(mut self, dir: PathBuf) -> Self {
        self.trash_dir = Some(dir);
        self
    }

    /// Send deleted presets to the desktop's home trash, where a file manager can restore them
    /// (0.4.0 audit #16).
    #[must_use]
    pub fn with_home_trash(mut self) -> Self {
        self.trash_dir = None;
        self
    }

    /// Where user presets are saved.
    #[must_use]
    pub fn user_dir(&self) -> &Path {
        &self.user_dir
    }

    /// Where factory presets are looked for, in search order — what `crossfx --self-test` reads
    /// back, so it checks the directories this store will actually use rather than a copy of the
    /// list that could drift from it.
    #[must_use]
    pub fn factory_dirs(&self) -> &[PathBuf] {
        &self.factory_dirs
    }

    /// Re-scan every directory. Unreadable directories are skipped, not fatal: a missing factory
    /// directory must not stop the application from starting.
    pub fn rescan(&mut self) {
        let mut factory = Vec::new();
        for dir in &self.factory_dirs {
            collect::<F>(dir, PresetSource::Factory, &mut factory);
        }
        let mut user = Vec::new();
        collect::<F>(&self.user_dir, PresetSource::User, &mut user);

        let mut entries = merge(factory, user);
        for entry in &mut entries {
            entry.modified = self
                .own_autosave_slots(&entry.name)
                .any(|slot| slot.is_file());
        }
        self.entries = entries;
    }

    /// The presets, in display order.
    #[must_use]
    pub fn entries(&self) -> &[PresetEntry] {
        &self.entries
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub fn find(&self, name: &str) -> Option<&PresetEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    #[must_use]
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.name == name)
    }

    /// Load a preset by name, preferring its autosave when it has one of its own.
    ///
    /// Returns the preset and whether it came from the autosave, which is what makes the GUI show
    /// the `*` marker.
    ///
    /// # Errors
    /// The name is not in the list, or the file it names cannot be read.
    pub fn load(&self, name: &str) -> Result<(F, bool), F::Error> {
        let entry = self.find(name).ok_or_else(|| F::unknown(name))?;

        if let Some(autosave) = self.own_autosave_slots(name).find(|slot| slot.is_file()) {
            match F::load(&autosave) {
                Ok(mut preset) => {
                    // The autosave carries the edited values but the canonical name.
                    preset.set_name(&entry.name);
                    return Ok((preset, true));
                }
                Err(err) => log::warn!(
                    "{}: {err}; falling back to the original",
                    autosave.display()
                ),
            }
        }

        let mut preset = F::load(&entry.path)?;
        preset.set_name(&entry.name);
        Ok((preset, false))
    }

    /// Load a preset as last **saved**: its own file, never its autosave.
    ///
    /// What an export hands on and a rename moves — both are about the file the name stands for,
    /// and unsaved edits are not what that name says.
    ///
    /// # Errors
    /// The name is not in the list, or its file cannot be read.
    pub fn load_saved(&self, name: &str) -> Result<F, F::Error> {
        let entry = self.find(name).ok_or_else(|| F::unknown(name))?;
        let mut preset = F::load(&entry.path)?;
        preset.set_name(&entry.name);
        Ok(preset)
    }

    /// Path of the autosave shadow copy for a preset: `AutoSave/` and the name's file
    /// ([`Store::file_name`]), or where 0.3.0 kept it when only that is there.
    ///
    /// 0.3.0 filed an autosave under the name exactly as written — `AutoSave/Rock:Live.fac` — so
    /// the unsaved edits it left of a preset whose name has a character a file name cannot keep
    /// are found there until the next autosave or save moves them to the name's own file
    /// ([`Store::autosave`], [`Store::clear_autosave`]). Without that they were orphaned: the
    /// preset loaded as last saved, without its `*`, and the edits never came back. Only on Unix,
    /// where 0.3.0 ran (`legacy_autosave_path` says why).
    ///
    /// The path, whatever the volume finds there: on Windows it can be the 8.3 short name of
    /// another preset's autosave, which the store then neither reads as this name's edits nor
    /// removes (`own_autosave_slots`) nor writes over ([`Store::autosave`]).
    #[must_use]
    pub fn autosave_path(&self, name: &str) -> PathBuf {
        let path = self.sanitised_autosave_path(name);
        if !path.is_file()
            && let Some(legacy) = self.legacy_autosave_path(name)
            && legacy.is_file()
        {
            return legacy;
        }
        path
    }

    /// `AutoSave/` and the sanitised name: where an autosave is written.
    fn sanitised_autosave_path(&self, name: &str) -> PathBuf {
        self.autosave_dir
            .join(format!("{}.{}", sanitise_preset_name(name), F::EXTENSION))
    }

    /// `AutoSave/` and the name as written, where 0.3.0 kept a preset's autosave: `None` when that
    /// is the sanitised path itself, when the name is no file name at all, and on any system but
    /// Unix.
    ///
    /// The name is the one inside the preset's file, and a file put in the directory by hand can
    /// carry any. Joined as written, a name that is a path reached out of `AutoSave/`, and the
    /// file it found there was read as the preset's unsaved edits: listed with a `*`, removed for
    /// good by its next autosave or undo, sent to the trash by its delete, moved by its rename —
    /// the user's own preset beside `AutoSave/`, or anything else of theirs on the drive. On Unix,
    /// where 0.3.0 ran, `/` is the one separator, so a name without it (or a NUL) is one file name
    /// inside `AutoSave/`. Windows reads `\`, `..\`, a drive's `C:` and a leading `\` as path
    /// syntax as well, and 0.3.0 never ran there: there is no 0.3.0 autosave to find, so none is
    /// looked for.
    ///
    /// Inside `AutoSave/` it is never another preset's autosave: a sanitised stem holds none of
    /// the characters, the surrounding spaces or the length that make the name differ from it.
    fn legacy_autosave_path(&self, name: &str) -> Option<PathBuf> {
        if !cfg!(unix) || name.is_empty() || name.contains(['/', '\0']) {
            return None;
        }
        let legacy = self.autosave_dir.join(format!("{name}.{}", F::EXTENSION));
        (legacy != self.sanitised_autosave_path(name)).then_some(legacy)
    }

    /// Where `name`'s unsaved edits can be, in the order [`Store::autosave_path`] looks: its file
    /// in `AutoSave/`, then one 0.3.0 left under its name as written — each unless the volume
    /// finds another file there, one the name reaches only as that file's alias (`alias_of`).
    /// That file holds another preset's edits, and the name must neither read them as its own nor
    /// remove them; [`Store::autosave`] refuses to write over them.
    ///
    /// On Windows `AutoSave/ROCKCO~1.fac` is `AutoSave/Rock Concert Live.fac` once NTFS has given
    /// that file the 8.3 short name: a preset called `ROCKCO~1` was listed with Rock Concert Live's
    /// `*`, loaded its edits, and dropped them with its own undo, save or delete.
    fn own_autosave_slots(&self, name: &str) -> impl Iterator<Item = PathBuf> {
        std::iter::once(self.sanitised_autosave_path(name))
            .chain(self.legacy_autosave_path(name))
            .filter(|slot| alias_of(slot).is_none())
    }

    /// Stash the user's unsaved edits so they survive a preset switch or a restart.
    ///
    /// Written to the name's own file in `AutoSave/`; a 0.3.0 autosave of the same preset under
    /// its name as written, whose edits these now carry, goes.
    ///
    /// Never over another file the volume finds under that name (`alias_of`): on Windows,
    /// `AutoSave/ROCKCO~1.fac` is Rock Concert Live's autosave once NTFS has given it that 8.3
    /// short name, and writing there replaced Rock Concert Live's unsaved edits with
    /// `ROCKCO~1`'s. That stash is refused instead, and the edits are left with the caller; the
    /// name is free again once the other preset's are saved or undone.
    ///
    /// # Errors
    /// The preset's name leaves nothing to file it under, its file in `AutoSave/` is another
    /// file's alias, or the file cannot be written.
    pub fn autosave(&self, preset: &F) -> Result<(), F::Error> {
        let name = preset.name();
        let file = Self::file_name(name)?;
        let path = self.autosave_dir.join(&file);
        if let Some(found) = alias_of(&path) {
            return Err(F::shared_file(
                name,
                &self.alias_holder(&found, name),
                &file,
            ));
        }
        preset.save(&path)?;
        if let Some(legacy) = self.legacy_autosave_path(name) {
            remove_if_there(&legacy);
        }
        Ok(())
    }

    /// Drop a preset's autosave, clearing its modified marker: the one under its own file and any
    /// 0.3.0 left under its name as written — never another preset's that the name reaches only as
    /// an alias (`own_autosave_slots`).
    pub fn clear_autosave(&mut self, name: &str) {
        for slot in self.own_autosave_slots(name) {
            remove_if_there(&slot);
        }
        if let Some(entry) = self.entries.iter_mut().find(|e| e.name == name) {
            entry.modified = false;
        }
    }

    /// Save a preset under a name, as a user preset, and refresh the list.
    ///
    /// Returns the path it was written to. Saving under a name already in the list is the
    /// overwrite: the user's copy replaces a factory one, or their own earlier one with a `.bak`
    /// kept. Saving under a name that would take *another* listed preset's file is refused,
    /// because that overwrite is one the list could never show: `Mu:sic` beside `Music`
    /// (`file_name` below says why one file can have two names), `ROCK` beside `Rock` on Windows,
    /// whose NTFS folds case, and any name whose file is already there and listed under the name
    /// inside it — a hand-copied `Rock.fac` that calls itself "Rock Concert", which a save of
    /// "Rock" replaced, leaving it in a `.bak` the list does not show. So is a name under which the
    /// volume finds a file of another name, listed or not: on Windows `ROCKCO~1` once NTFS has
    /// given `Rock Concert Live.fac` that 8.3 short name (`alias_of`).
    ///
    /// A user preset is overwritten in the file it is listed from, whatever that file is called.
    /// 0.3.0 filed a preset saved from the window under its name as written — `Rock:Live.fac` —
    /// and a hand-copied file can be called anything; writing the name's own file instead
    /// (`RockLive.fac`) left the list on the old one, which still won, so the save looked lost,
    /// and its unsaved edits, cleared by the save, were gone with it.
    ///
    /// # Errors
    /// The name leaves nothing to file it under, its file belongs to a preset of a different
    /// name or is another file's alias, or the file cannot be written.
    pub fn save_as(&mut self, preset: &F, name: &str) -> Result<PathBuf, F::Error> {
        let path = match self.find(name) {
            Some(entry) if entry.source == PresetSource::User => entry.path.clone(),
            _ => {
                let file = Self::file_name(name)?;
                let path = self.user_dir.join(&file);
                if let Some(other) = self
                    .holder_of(&file, name)
                    .or_else(|| self.listed_from(&path, name))
                {
                    return Err(F::shared_file(name, &other.name, &file));
                }
                if let Some(found) = alias_of(&path) {
                    return Err(F::shared_file(
                        name,
                        &self.alias_holder(&found, name),
                        &file,
                    ));
                }
                path
            }
        };
        let mut to_save = preset.clone();
        to_save.set_name(name);
        // The user-facing overwrite: worth keeping the previous version, unlike the autosave.
        to_save.save_with_backup(&path)?;
        self.clear_autosave(name);
        self.rescan();
        Ok(path)
    }

    /// Delete a user preset: its file goes to the desktop's trash — the home trash, or the trash
    /// of its own filesystem when that is another one — where a file manager can restore it, or
    /// when no trash can take it, is set aside beside itself as
    /// `<file>.1.bak` or the next free number ([`trash::set_aside`]) (0.4.0 audit #16; the
    /// original deletes it for good). Factory presets are refused, as in the original.
    ///
    /// Its unsaved edits, the autosave, go the same way after it rather than being dropped: a
    /// preset with unsaved changes can be deleted, and those edits exist nowhere else. The trash
    /// records each file's own place, so restoring both puts the preset back in the list with its
    /// edits and its `*`. Should the autosave move nowhere at all, it is left where it is. Another
    /// preset's that the name reaches only as an alias stays too (`own_autosave_slots`).
    ///
    /// Returns where the preset's file went, or `None` for a name the list does not hold.
    ///
    /// # Errors
    /// The preset is a factory one, or its file can be neither trashed nor renamed.
    pub fn delete(&mut self, name: &str) -> Result<Option<Discarded>, F::Error> {
        let Some(entry) = self.find(name) else {
            return Ok(None);
        };
        if entry.source == PresetSource::Factory {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("{name} is a factory preset"),
            )
            .into());
        }
        let path = entry.path.clone();
        let discarded = self.discard(&path)?;
        // Its own autosave, and one 0.3.0 left under its name as written.
        let autosaves = self
            .own_autosave_slots(name)
            .filter(|autosave| autosave.is_file())
            .collect::<Vec<_>>();
        for autosave in autosaves {
            match self.discard(&autosave) {
                Ok(Discarded::Trash(to) | Discarded::Backup(to)) => {
                    log::info!("{name}: its unsaved changes went to {}", to.display());
                }
                Err(err) => log::warn!("{}: left where it is: {err}", autosave.display()),
            }
        }
        self.rescan();
        Ok(Some(discarded))
    }

    /// [`trash::discard`] into this store's trash.
    fn discard(&self, path: &Path) -> std::io::Result<Discarded> {
        match &self.trash_dir {
            Some(dir) => trash::discard_into(path, dir),
            None => trash::discard(path),
        }
    }

    /// Give the user preset `old` the name `new` (0.4.0 audit #19).
    ///
    /// A move, not a copy: the file is rewritten in place with the new name inside it and then
    /// renamed with [`std::fs::rename`], and its autosave, if it has one, is renamed the same way.
    /// The original saves a copy under the new name and deletes the old file
    /// (`FxController.cpp:1244-1276`), and on a filesystem that folds case — a FAT or exFAT stick,
    /// a case-insensitive ext4 directory — `rock.fac` and `Rock.fac` are one file, so renaming
    /// `rock` to `Rock` that way wrote the new file over the old one and then deleted it. Renaming
    /// the one file cannot. The name inside is written first, so that a rename that then fails
    /// still leaves a preset the list shows under its new name.
    ///
    /// A name that would take another listed preset's file is refused, as [`Store::save_as`]
    /// refuses it — a file already under the new name that another preset is listed from
    /// included, which would otherwise be set aside and leave the list; the preset's own file is
    /// not another's, so a change of case alone is allowed. An unlisted file already under the
    /// new name is set aside as `<file>.1.bak` or the next free number ([`trash::set_aside`]),
    /// never over a `.bak` already there. An autosave already under the new name, when the preset
    /// brings none of its own, goes to the trash the way a deleted preset's does: it holds another
    /// preset's edits, never this one's.
    ///
    /// A file the volume finds under the new name but keeps under another — on Windows an 8.3
    /// short name, `ROCKCO~1.fac` for `Rock Concert Live.fac` (`alias_of`) — is no file of the new
    /// name's to set aside or to trash. One in the user directory refuses the rename, and so does
    /// another preset's autosave when this preset brings unsaved edits to put in its place; without
    /// them the rename goes through and the other's autosave stays. The preset's own file and
    /// autosave are its own under any name: renamed to its own short name, it moves.
    ///
    /// # Errors
    /// `old` is not a user preset, `new` leaves nothing to file it under or is another preset's
    /// file or another file's alias, or the files cannot be written or moved.
    pub fn rename(&mut self, old: &str, new: &str) -> Result<(), F::Error> {
        let entry = self.find(old).ok_or_else(|| F::unknown(old))?;
        if entry.source == PresetSource::Factory {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("{old} is a factory preset"),
            )
            .into());
        }
        let from = entry.path.clone();
        let file = Self::file_name(new)?;
        let to = self.user_dir.join(&file);
        // Another preset of exactly the new name, one whose file the new name would take, or one
        // listed from the file already under it.
        if let Some(other) = self
            .find(new)
            .filter(|other| other.name != old)
            .or_else(|| self.holder_of(&file, new).filter(|other| other.name != old))
            .or_else(|| self.listed_from(&to, old))
        {
            return Err(F::shared_file(new, &other.name, &file));
        }
        // A file the new name reaches only as another's alias: one in the user directory, which
        // would be set aside under the new name's `.bak`, or another preset's unsaved edits, which
        // this one's would replace. Not this preset's own file or autosave, whatever it is called.
        let old_autosave = self.own_autosave_slots(old).find(|slot| slot.is_file());
        let new_autosave = self.autosave_dir.join(&file);
        let reached = alias_of(&to)
            .filter(|_| !same_file(&from, &to))
            .or_else(|| {
                let own = old_autosave.as_deref()?;
                alias_of(&new_autosave).filter(|_| !same_file(own, &new_autosave))
            });
        if let Some(found) = reached {
            return Err(F::shared_file(new, &self.alias_holder(&found, new), &file));
        }

        let mut preset = F::load(&from)?;
        preset.set_name(new);
        // Another file already there that is not this one: kept, beside every other `.bak`.
        if to != from && to.exists() && !same_file(&from, &to) {
            trash::set_aside(&to)?;
        }
        preset.save(&from)?;
        std::fs::rename(&from, &to)?;

        // An autosave already under the new name is not this preset's: a 0.3.0 microphone curve
        // stashed under a voice preset's name, the edits of a preset deleted by hand or one whose
        // autosave the trash would not take. Left there, it would be read as the renamed preset's
        // unsaved changes — the `*` on a preset that had none, and its curve loaded in place of
        // the file's on the next pick or start (0.4.0 review FA). It goes where a deleted preset's
        // edits go, not over them; so does one 0.3.0 left under the new name as written, which
        // [`Store::autosave_path`] would find the same way. Another preset's autosave that the new
        // name reaches only as an alias is no leftover of the new name's, and stays.
        let mut leftovers = Vec::new();
        if let Some(old_autosave) = old_autosave {
            let moved = F::load(&old_autosave).and_then(|mut stash| {
                stash.set_name(new);
                stash.save(&old_autosave)?;
                std::fs::rename(&old_autosave, &new_autosave)?;
                Ok(())
            });
            if let Err(err) = moved {
                log::warn!("{}: {err}", old_autosave.display());
            }
        } else {
            leftovers.push(new_autosave);
        }
        leftovers.extend(self.legacy_autosave_path(new));
        for leftover in leftovers
            .into_iter()
            .filter(|leftover| leftover.is_file() && alias_of(leftover).is_none())
        {
            if let Err(err) = self.discard(&leftover) {
                log::warn!("{}: {err}; removing it", leftover.display());
                if let Err(err) = std::fs::remove_file(&leftover) {
                    log::warn!("{}: {err}", leftover.display());
                }
            }
        }
        self.rescan();
        Ok(())
    }

    /// Import a preset file into the user directory, rejecting anything that does not parse.
    ///
    /// Returns the imported preset's name, which is the file's stem: a file someone chose to
    /// import is a file they know by its name on disk. The stem is a new name, so it is made one
    /// the original and CrossFX alike can read ([`new_preset_name`]), and that is the name written
    /// inside the copy too (0.4.0 audit #15): a file called `a:b.fac` used to be imported as
    /// `ab.fac` with `a:b` inside it, a name the original could never file.
    ///
    /// An import adds a preset and never replaces one of the user's: a name a user preset already
    /// has, or a file already in the user directory — which on a filesystem that folds case is
    /// also `rock.fac` when `Rock.fac` is there, and on Windows `ROCKCO~1.fac` when NTFS has given
    /// `Rock Concert Live.fac` that 8.3 short name — is refused rather than handed to
    /// [`Store::save_as`], whose overwrite would leave the earlier preset only in a `.bak` the
    /// list does not show. A factory preset's name can still be taken, as a save can take it: the
    /// user's copy then stands in for the factory one. The controller refuses that too, and any
    /// name already listed in another case, before it asks.
    ///
    /// # Errors
    /// The file does not parse, its name leaves nothing to file it under, is a user preset's or
    /// another listed preset's file, or the copy cannot be saved into the user directory.
    pub fn import(&mut self, source: &Path) -> Result<String, F::Error> {
        let preset = F::load(source)?;
        let name = new_preset_name(
            source
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_else(|| preset.name()),
        );
        let to = self.user_dir.join(Self::file_name(&name)?);
        if self
            .find(&name)
            .is_some_and(|entry| entry.source == PresetSource::User)
            || to.exists()
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("{name}: {} is already there", to.display()),
            )
            .into());
        }
        self.save_as(&preset, &name)?;
        Ok(name)
    }

    /// Export a preset to a directory chosen by the user.
    ///
    /// What is exported is the preset as last **saved**, never its autosave: an export is a file
    /// someone hands on under the preset's name, and unsaved edits are not what that name says
    /// (the original's PR #155, `FxController.cpp:1411`, which exports `preset.path`). Save first
    /// to export the edits.
    ///
    /// A file the volume finds in `dir` under the name but keeps under another (`alias_of`) — on
    /// Windows `ROCKCO~1.fac` once NTFS has given `Rock Concert Live.fac` that 8.3 short name — is
    /// not replaced: the export of `ROCKCO~1` would have taken Rock Concert Live's place.
    ///
    /// # Errors
    /// The name is unknown, leaves nothing to file it under, its file in `dir` is another file's
    /// alias, or the file cannot be written.
    pub fn export(&self, name: &str, dir: &Path) -> Result<PathBuf, F::Error> {
        self.export_with(name, dir, EndBands::Shifted)
    }

    /// [`Store::export`] with the end bands put as `end_bands` says ([`EndBands`]).
    ///
    /// # Errors
    /// As [`Store::export`].
    pub fn export_with(
        &self,
        name: &str,
        dir: &Path,
        end_bands: EndBands,
    ) -> Result<PathBuf, F::Error> {
        let preset = self.load_saved(name)?.exported(end_bands);
        let file = Self::file_name(name)?;
        let path = dir.join(&file);
        if let Some(found) = alias_of(&path) {
            return Err(F::shared_file(
                name,
                &self.alias_holder(&found, name),
                &file,
            ));
        }
        preset.save(&path)?;
        Ok(path)
    }

    /// The file a name is stored under: the sanitised stem and the format's extension.
    ///
    /// Every route from a name to a filename goes through here and so through
    /// [`sanitise_preset_name`] — the same function the command line applies — which is what
    /// keeps a preset saved from the window and one saved from a script in the same file. The
    /// one exception is a user preset already listed, which [`Store::save_as`] saves over in the
    /// file it is listed from, whatever that is called.
    ///
    /// The sanitiser is not one-to-one, and that is the cost of keeping the typed name inside
    /// the file: `Mu:sic` and `Music` are two names and one stem, so two listed presets can lay
    /// claim to one file. Between two user presets it is the same `.fac`; beside a factory
    /// preset, which lives under a numbered file, it is the same autosave and the same export,
    /// so an edit to one would surface as the other's unsaved change. [`Self::save_as`] refuses
    /// to create either state, through `holder_of`. The state can still arrive from files put in
    /// the directory by hand, which `rescan` lists as the two names they are; the shared autosave
    /// slot is then the one thing the list cannot tell apart. Stems are compared the way the
    /// filesystem compares them: exactly on Unix, and on Windows whatever their case, since NTFS
    /// folds it and `ROCK.fac` is `Rock.fac` there (`same_file_name`). What no comparison of
    /// names can see, the volume is asked once a file is there: on Windows `ROCKCO~1.fac` is
    /// `Rock Concert Live.fac` after NTFS has given it that 8.3 short name (`alias_of`). The
    /// case-insensitive rule between *names* is the controller's, as the sanitiser's own note says.
    ///
    /// Public so a caller that has to know where a preset will land — the export window, asking
    /// about a file already there before it writes — asks the store rather than keeping a copy of
    /// the rule that could drift from it.
    ///
    /// # Errors
    /// The name leaves nothing to file it under.
    pub fn file_name(name: &str) -> Result<String, F::Error> {
        let stem = sanitise_preset_name(name);
        if stem.is_empty() {
            return Err(F::empty_name());
        }
        Ok(format!("{stem}.{}", F::EXTENSION))
    }

    /// The listed preset, if any, other than `name` itself whose file is `file`: the file its
    /// name maps to, compared as the filesystem compares file names (`same_file_name`). On
    /// Windows a name that differs from a listed one only in case is that preset's file, autosave
    /// and export, whether or not the file has been written yet. A name that is the 8.3 short name
    /// of another's is not: the volume gives that alias only to a file that is there, and
    /// `alias_of` finds it then.
    ///
    /// An entry whose own name sanitises to nothing has no file to hold, so it can never be the
    /// holder; the `Ok` filter is that, not an error swallowed.
    fn holder_of(&self, file: &str, name: &str) -> Option<&PresetEntry> {
        self.entries.iter().find(|entry| {
            entry.name != name
                && Self::file_name(&entry.name).is_ok_and(|held| same_file_name(&held, file))
        })
    }

    /// Whose file `found` is — a file the volume finds under `name`'s but keeps under its own name,
    /// `found` (`alias_of`) — as a refusal names it: the listed preset other than `name` whose
    /// file that is, or else the file itself, which may be one no preset is listed from.
    fn alias_holder(&self, found: &str, name: &str) -> String {
        self.holder_of(found, name)
            .map_or_else(|| found.to_owned(), |entry| entry.name.clone())
    }

    /// The listed preset, if any, other than `name` itself that is listed from the file at
    /// `path`: a file already there under the name a save or a rename is about to write, which
    /// would replace it or set it aside, and the list would lose a preset it showed.
    ///
    /// `holder_of` goes by the file each listed *name* maps to, and that is not always the file it
    /// is listed from: a file put in the directory by hand is listed by the name inside it,
    /// whatever the file is called — `Rock.fac` holding "Rock Concert". The filesystem is asked
    /// rather than the names compared (`same_file`), so its own folding of case decides what is
    /// one file.
    fn listed_from(&self, path: &Path, name: &str) -> Option<&PresetEntry> {
        if !path.exists() {
            return None;
        }
        self.entries
            .iter()
            .find(|entry| entry.name != name && same_file(&entry.path, path))
    }
}

/// Whether two file names in one directory name one file: exactly, on Unix, where a file name is
/// the bytes it is written in. Where a filesystem there folds case after all — a FAT stick, an
/// ext4 directory with case folding on — a file already under the other name is still found, by
/// `listed_from`, which asks the filesystem rather than comparing names.
#[cfg(unix)]
fn same_file_name(a: &str, b: &str) -> bool {
    a == b
}

/// Whether two file names in one directory name one file: on Windows, letter by letter as NTFS
/// upcases them (`upcase`), so `ROCK.fac` is `Rock.fac`.
///
/// Names alone cannot tell the rest. Where a volume makes 8.3 short names, as a Windows system
/// drive does by default, NTFS gives a long name a second one, `ROCKCO~1.FAC` for
/// `Rock Concert Live.fac`, and opens the file under either: `ROCKCO~1.fac` is then that file,
/// although no letter of the two names matches. Which long name has which alias only the volume
/// knows, and only once the file is there; `alias_of` asks it.
#[cfg(not(unix))]
fn same_file_name(a: &str, b: &str) -> bool {
    a.chars().map(upcase).eq(b.chars().map(upcase))
}

/// `c` as NTFS upcases it to compare file names, as near as the standard library's case tables
/// can tell, against the table Windows 7 through 11 write: on a volume that carries it, two names
/// this makes one are one file, or two when they differ only in a case pair newer than the table,
/// and two names it keeps apart are never one file.
///
/// NTFS looks each UTF-16 unit of a name up in a table the volume carries, `$UpCase`, written by
/// the Windows that formatted it. The one Windows 7 and later write pairs a letter with an
/// uppercase only when each is the other's: `i` with `I`, `σ` with `Σ`. Unicode uppercases
/// further, to a capital that lowercases to another letter — the dotless `ı` and the long `ſ` to
/// `I` and `S`, the micro sign `µ` to the Greek capital `Μ`, the final `ς` to `Σ`, the titlecase
/// `ǅ` to `Ǆ` — and that table tells each such letter from that capital and from the capital's own
/// small letter: `Kayıt.fac` is one file and `Kayit.fac` another. Taking every one-letter
/// uppercase, as this did at first, refused the second name of such a pair, which the controller
/// lets through. So a letter is upcased only when its uppercase lowercases back to it, and only
/// inside the Basic Multilingual Plane: a character beyond it is two units, and the table maps
/// each of them to itself.
///
/// The Greek small letters with iota subscript go wrong the other way. The standard library
/// uppercases them to two letters, `ᾳ` to `ΑΙ`, which that rule cannot follow, while the table
/// pairs each with the one capital that lowercases to it, `ᾼ`; the match below pairs them as NTFS
/// does. Without it `ᾳ.fac` and `ᾼ.fac` would be two names here for what NTFS makes one file.
///
/// The table's age is what cannot be known here. Windows' is older than the Unicode the standard
/// library follows, so the case pairs added since — Georgian's capitals, Cherokee's small letters,
/// the Greek capital yot `Ϳ`, the Glagolitic `Ⱟ` and `ⱟ`, and newer Latin, Cyrillic and Coptic
/// letters — fold here and not on the volume: a name that differs from a listed one only in those
/// is refused although NTFS would keep it apart. That is the side to be wrong on, and the
/// controller's name check, which compares names in lowercase, refuses those names first.
///
/// A volume formatted by Windows XP or Vista carries an older table, which also upcases the final
/// `ς` to `Σ`: there `Λόγος.fac` and `Λόγοσ.fac` are one file, and this keeps them apart. Folding
/// `ς` with `σ` here would refuse `Λόγοσ` beside `Λόγος` on every newer volume, where the two are
/// files of their own and the controller lets the name through; so the store asks such a volume
/// instead, once a file is there under either name: `listed_from` refuses a save or a rename onto
/// a listed preset's file, and `alias_of` keeps the other name off any file the volume finds under
/// it. XP's table leaves the letters with iota subscript apart from their capitals as well, which
/// only makes this refuse names that volume would keep apart.
#[cfg(not(unix))]
fn upcase(c: char) -> char {
    // `ᾀ`–`ᾇ`, `ᾐ`–`ᾗ` and `ᾠ`–`ᾧ`, whose capitals are eight places on, and `ᾳ`, `ῃ` and `ῳ`,
    // whose capitals are nine.
    let iota_subscript = match c {
        '\u{1F80}'..='\u{1F87}' | '\u{1F90}'..='\u{1F97}' | '\u{1FA0}'..='\u{1FA7}' => 8,
        '\u{1FB3}' | '\u{1FC3}' | '\u{1FF3}' => 9,
        _ => 0,
    };
    if iota_subscript > 0 {
        return char::from_u32(u32::from(c) + iota_subscript).unwrap_or(c);
    }
    let mut upper = c.to_uppercase();
    let (Some(u), None) = (upper.next(), upper.next()) else {
        return c;
    };
    let one_unit_each = c.len_utf16() == 1 && u.len_utf16() == 1;
    if one_unit_each && u.to_lowercase().eq([c]) {
        u
    } else {
        c
    }
}

/// Remove `path` when it is there, saying in the log when that fails.
fn remove_if_there(path: &Path) {
    if path.exists()
        && let Err(err) = std::fs::remove_file(path)
    {
        log::warn!("{}: {err}", path.display());
    }
}

/// Whether two paths are one file: the same inode, which on a filesystem that folds case is what
/// `rock.fac` and `Rock.fac` are.
#[cfg(unix)]
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// Whether two paths are one file. NTFS folds case, and `canonicalize` hands back the name as the
/// volume spells it, so `rock.fac` and `Rock.fac` resolve to the same path.
#[cfg(not(unix))]
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// The name of the file the volume finds at `path` when that is not `path`'s own: the file `path`
/// opens is kept under another name, as `same_file_name` compares names, and its directory lists
/// nothing under `path`'s. NTFS opens a file under its 8.3 short name — `ROCKCO~1.fac` is
/// `Rock Concert Live.fac` once the volume has given it that alias — and a volume formatted by XP
/// or Vista finds `Λόγος.fac` under `Λόγοσ.fac` (`upcase`). A write there would replace that
/// other file, and a read would take it for this name's.
///
/// `None` when nothing is there, when the file is the name's own in another case, and when the
/// volume cannot be asked. `canonicalize` names the file at the end of a link, so a name it does
/// not match is looked for in the directory before it counts: a link under the name stays the
/// name's own whatever the file it leads to is called.
#[cfg(not(unix))]
fn alias_of(path: &Path) -> Option<String> {
    let own = path.file_name()?.to_str()?;
    let found = std::fs::canonicalize(path).ok()?;
    let found = found.file_name()?.to_string_lossy().into_owned();
    if same_file_name(&found, own) {
        return None;
    }
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let listed = std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|listed| same_file_name(listed, own))
        })
    });
    (!listed).then_some(found)
}

/// Nothing, on Unix, where a name is its bytes (`same_file_name`). Where a filesystem there finds
/// a file under another name after all — another case of it on a FAT stick or in a case-folding
/// ext4 directory, or a FAT short name, which Linux's `vfat` opens as Windows does — `listed_from`
/// still finds a listed preset's file by asking the filesystem (`same_file`). The autosave and the
/// export a FAT short name reaches are not looked after: no Unix call names the file a path opens,
/// and a scan of the directory by inode would still have to tell a short name from the other
/// spellings such a filesystem folds together, case among them, without its rules. A preset folder
/// on FAT is the rare case that leaves.
#[cfg(unix)]
fn alias_of(_path: &Path) -> Option<String> {
    None
}

/// The order the list is in: numbered factory files first, by number; the rest of the factory
/// presets by name; then the user's own, by name.
type SortKey = (u8, u32, String);

/// Factory presets first — in the order the vendor numbered their files, then the unnumbered rest
/// by name — and the user's presets after all of them, by name.
///
/// The numbering is the only record of the intended order — `1.fac` is General, `2.fac` is Music —
/// and it is more useful than sorting those twelve alphabetically. The original lists presets in
/// filesystem-glob order, which is arbitrary, one directory after the other: the application's
/// presets, then the user's; this is the deterministic version of the same intent.
///
/// The user's used to be sorted in among the unnumbered factory presets, so "My Preset" sat
/// between "Modern Rock" and "Panserotaliya", the window's rule between factory and user presets
/// (`FxView.cpp:205-225`) fell in the middle of factory ones, and the list read otherwise than the
/// tray's, which groups them (E6b).
fn sort_key(entry: &PresetEntry) -> SortKey {
    let numbered_file = entry
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.parse::<u32>().ok());
    match (numbered_file, entry.source) {
        (Some(n), PresetSource::Factory) => (0, n, String::new()),
        (None, PresetSource::Factory) => (1, 0, entry.name.to_lowercase()),
        (_, PresetSource::User) => (2, 0, entry.name.to_lowercase()),
    }
}

/// One entry per name, with the user's copy winning, in display order.
///
/// The user's copy replaces the factory one *in place*: it takes the factory entry's sort key,
/// so a user "General" still leads the list where the factory General did, rather than sinking
/// into the alphabetical rest. 0.3.0 sorted first and then removed *consecutive* duplicates,
/// which is why a user preset named like a numbered factory one survived beside it — the two
/// sorted apart, both were listed, and `find` returned the factory copy. A name found in two
/// factory directories (the source tree and an installed package, say) is listed from the first;
/// two user files carrying the same name are listed from the first in path order, with a warning.
fn merge(factory: Vec<PresetEntry>, user: Vec<PresetEntry>) -> Vec<PresetEntry> {
    let mut keyed: Vec<(SortKey, PresetEntry)> = Vec::with_capacity(factory.len() + user.len());
    for entry in factory {
        if keyed.iter().any(|(_, kept)| kept.name == entry.name) {
            continue;
        }
        keyed.push((sort_key(&entry), entry));
    }
    for entry in user {
        match keyed.iter_mut().find(|(_, kept)| kept.name == entry.name) {
            Some((_, kept)) if kept.source == PresetSource::Factory => *kept = entry,
            Some((_, kept)) => log::warn!(
                "{}: another user preset is already named {:?} ({}); skipping",
                entry.path.display(),
                entry.name,
                kept.path.display()
            ),
            None => keyed.push((sort_key(&entry), entry)),
        }
    }
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    keyed.into_iter().map(|(_, entry)| entry).collect()
}

fn collect<F: PresetFile>(dir: &Path, source: PresetSource, out: &mut Vec<PresetEntry>) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    let mut found = Vec::new();
    for entry in read_dir.flatten() {
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case(F::EXTENSION))
        {
            continue;
        }
        // The original calls getPresetInfo() and skips any file whose name decodes empty; parsing
        // is the equivalent check and also rejects files that are not presets at all.
        match F::load(&path) {
            Ok(preset) => {
                // The name lives INSIDE the file, not in its filename. The factory presets are
                // shipped as `1.fac`..`12.fac` but call themselves General, Music, Voice and so on
                // — `getPresetInfo()` reads the file (`DfxDspPreset.cpp:363-397`) and the combo box
                // shows what it finds. Only fall back to the stem for a file with no name in it,
                // which the original skips outright.
                let name = if preset.name().trim().is_empty() {
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default()
                        .to_owned()
                } else {
                    preset.name().to_owned()
                };
                if name.is_empty() {
                    log::warn!("{}: no preset name; skipping", path.display());
                    continue;
                }
                found.push(PresetEntry {
                    name,
                    path,
                    source,
                    modified: false,
                });
            }
            Err(err) => log::warn!("{}: {err}; skipping", path.display()),
        }
    }
    // `read_dir` order is whatever the filesystem feels like; the list must not depend on it.
    found.sort_by(|a, b| a.path.cmp(&b.path));
    out.extend(found);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxsound_core::test_support::ScratchDir;

    fn assets() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/presets")
            .canonicalize()
            .expect("assets/presets exists")
    }

    fn store_in(tmp: &Path) -> PresetStore {
        let assets = assets();
        let mut store = PresetStore::with_dirs(
            vec![assets.join("Factsoft"), assets.join("BonusPresets")],
            tmp.to_path_buf(),
        );
        store.rescan();
        store
    }

    /// A new directory of the test's own, removed when the test ends — also when it panics.
    fn tempdir(tag: &str) -> ScratchDir {
        ScratchDir::new(&format!("preset-test-{tag}"))
    }

    /// The twelve factory presets, in the order their filenames number them.
    const FACTORY: [&str; 12] = [
        "General",
        "Music",
        "Voice",
        "Volume Boost",
        "Gaming",
        "Classic Processing",
        "Light Processing",
        "Bass Boost",
        "Streaming Video",
        "Movies",
        "TV",
        "Transcription",
    ];

    #[test]
    fn finds_every_shipped_preset() {
        let tmp = tempdir("enumerate");
        let store = store_in(&tmp);
        assert!(
            store.entries().len() >= 30,
            "found {}",
            store.entries().len()
        );
        assert!(store.find("Jazz").is_some());
        assert!(
            store
                .entries()
                .iter()
                .all(|e| e.source == PresetSource::Factory)
        );
    }

    #[test]
    fn a_presets_name_comes_from_the_file_not_the_filename() {
        // The factory presets ship as 1.fac..12.fac and call themselves General, Music, Voice…
        // Showing "1" in the combo box was a real bug: the original reads the name out of the file.
        let tmp = tempdir("names");
        let store = store_in(&tmp);
        for name in FACTORY {
            let entry = store
                .find(name)
                .unwrap_or_else(|| panic!("{name} is missing from the list"));
            assert_eq!(entry.source, PresetSource::Factory);
            assert!(
                entry
                    .path
                    .file_stem()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .parse::<u32>()
                    .is_ok(),
                "{name} should come from a numbered file"
            );
        }
        for digit in ["1", "2", "12"] {
            assert!(
                store.find(digit).is_none(),
                "{digit} must not be shown as a name"
            );
        }
    }

    #[test]
    fn factory_presets_come_first_in_the_order_their_files_are_numbered() {
        let tmp = tempdir("ordering");
        let store = store_in(&tmp);
        let names: Vec<_> = store.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            &names[..FACTORY.len()],
            &FACTORY[..],
            "the twelve factory presets must lead the list in file order"
        );
        // Everything after them is the bonus set, sorted by name.
        let rest: Vec<String> = names[FACTORY.len()..]
            .iter()
            .map(|n| n.to_lowercase())
            .collect();
        let mut sorted = rest.clone();
        sorted.sort();
        assert_eq!(rest, sorted, "the rest of the list must be alphabetical");
    }

    #[test]
    fn the_users_presets_come_after_every_factory_one_by_name() {
        // E6b: "My Preset" was listed between "Modern Rock" and "Panserotaliya (Quizal)", so the
        // window's rule between factory and user presets fell among factory ones.
        let tmp = tempdir("user-after-factory");
        let mut store = store_in(&tmp);
        let (general, _) = store.load("General").expect("load General");
        for name in ["My Preset", "aardvark", "Zebra"] {
            store.save_as(&general, name).expect("save");
        }
        let entries = store.entries();
        let first_user = entries
            .iter()
            .position(|e| e.source == PresetSource::User)
            .expect("the user's presets are listed");
        assert!(
            entries[first_user..]
                .iter()
                .all(|e| e.source == PresetSource::User),
            "a factory preset after the first user one: {:?}",
            entries.iter().map(|e| &e.name).collect::<Vec<_>>()
        );
        let users: Vec<&str> = entries[first_user..]
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(users, ["aardvark", "My Preset", "Zebra"]);
    }

    #[test]
    fn a_user_preset_shadows_a_factory_one_of_the_same_name() {
        let tmp = tempdir("shadow");
        let mut store = store_in(&tmp);
        let (mut jazz, _) = store.load("Jazz").expect("load Jazz");
        jazz.set_effect(fxsound_core::Effect::Bass, 1.0);
        store.save_as(&jazz, "Jazz").expect("save");

        let entry = store.find("Jazz").expect("Jazz still listed");
        assert_eq!(entry.source, PresetSource::User);
        assert_eq!(
            store.entries().iter().filter(|e| e.name == "Jazz").count(),
            1
        );

        let (reloaded, _) = store.load("Jazz").expect("reload");
        assert_eq!(reloaded.effect(fxsound_core::Effect::Bass), 1.0);
    }

    #[test]
    fn a_user_preset_named_like_a_numbered_factory_one_wins_and_keeps_its_place() {
        // The 0.3.0 bug. `dedup_by` removes *consecutive* equals, and a user "General.fac" sorted
        // among the alphabetical rest while the factory General led the list by its file number:
        // both survived, and `find` returned the factory copy. "Jazz" above cannot see it — a
        // bonus preset and its shadow sort alike — so this one uses a numbered name.
        let tmp = tempdir("shadow-numbered");
        let mut store = store_in(&tmp);
        let (factory, _) = store.load("General").expect("load General");
        assert_ne!(
            factory.effect(fxsound_core::Effect::Bass),
            1.0,
            "the test needs a value the factory copy does not already hold"
        );
        let mut general = factory;
        general.set_effect(fxsound_core::Effect::Bass, 1.0);
        store.save_as(&general, "General").expect("save");

        assert_eq!(
            store
                .entries()
                .iter()
                .filter(|e| e.name == "General")
                .count(),
            1,
            "listed once"
        );
        let entry = store.find("General").expect("still listed");
        assert_eq!(
            entry.source,
            PresetSource::User,
            "and it is the user's copy"
        );
        assert_eq!(
            store.index_of("General"),
            Some(0),
            "which leads the list where the factory copy did"
        );
        let (reloaded, _) = store.load("General").expect("reload");
        assert_eq!(reloaded.effect(fxsound_core::Effect::Bass), 1.0);
    }

    #[test]
    fn the_same_factory_preset_in_two_directories_is_listed_once() {
        // `with_default_dirs` looks in the source tree and in the install prefixes, so a developer
        // with the package installed sees every factory preset from two places. The first wins.
        let tmp = tempdir("two-factory-dirs");
        let assets = assets();
        let copy = tmp.join("copy");
        std::fs::create_dir_all(&copy).expect("mkdir");
        std::fs::copy(assets.join("Factsoft/1.fac"), copy.join("1.fac")).expect("copy");
        let mut store =
            PresetStore::with_dirs(vec![assets.join("Factsoft"), copy], tmp.join("user"));
        store.rescan();

        assert_eq!(
            store
                .entries()
                .iter()
                .filter(|e| e.name == "General")
                .count(),
            1
        );
        assert!(store.find("General").unwrap().path.starts_with(&assets));
    }

    #[test]
    fn two_user_files_carrying_the_same_name_are_listed_once() {
        let tmp = tempdir("duplicate-user-name");
        let mut store = PresetStore::with_dirs(Vec::new(), tmp.join("user"));
        let preset = Preset {
            name: "Twice".into(),
            ..Preset::default()
        };
        crate::save(&preset, &tmp.join("user/a.fac")).expect("a");
        crate::save(&preset, &tmp.join("user/b.fac")).expect("b");
        store.rescan();

        assert_eq!(store.entries().len(), 1);
        assert!(
            store.entries()[0].path.ends_with("a.fac"),
            "the first in path order wins"
        );
    }

    #[test]
    fn an_unknown_name_is_reported_as_unknown() {
        // Not `Malformed { line: 0 }`: the string reaches the command line and D-Bus, and "line
        // 0: expected a known preset name" describes a file that was never opened.
        let tmp = tempdir("unknown");
        let store = store_in(&tmp);
        let err = store.load("No Such Preset").unwrap_err();
        assert!(
            matches!(&err, PresetError::Unknown(name) if name == "No Such Preset"),
            "{err}"
        );
        assert!(err.to_string().contains("No Such Preset"));
    }

    #[test]
    fn a_name_becomes_a_filename_through_the_one_sanitiser() {
        // 0.3.0 had two sanitisers: the command line stripped nine characters while this store
        // replaced three with underscores, so a name with a `:` typed in the window became a
        // file the original could not open. Both routes now go through
        // `sanitise_preset_name`; the name inside the file stays what was typed.
        let tmp = tempdir("sanitise");
        let mut store = PresetStore::with_dirs(Vec::new(), tmp.join("user"));
        let preset = Preset {
            name: "Mu:sic?".into(),
            ..Preset::default()
        };
        let path = store.save_as(&preset, "Mu:sic?").expect("save");
        assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("Music.fac"));
        assert_eq!(store.entries()[0].name, "Mu:sic?");

        let exported = store.export("Mu:sic?", &tmp).expect("export");
        assert_eq!(
            exported.file_name().and_then(|n| n.to_str()),
            Some("Music.fac")
        );
        assert!(
            store
                .autosave_path("Mu:sic?")
                .ends_with("AutoSave/Music.fac"),
            "{}",
            store.autosave_path("Mu:sic?").display()
        );
    }

    #[test]
    fn a_name_with_nothing_safe_in_it_is_refused() {
        // Stripping can leave nothing, and `.fac` is not a file anyone can find again.
        let tmp = tempdir("empty-name");
        let mut store = PresetStore::with_dirs(Vec::new(), tmp.join("user"));
        let preset = Preset {
            name: " : ".into(),
            ..Preset::default()
        };
        let err = store.save_as(&preset, " : ").unwrap_err();
        assert!(matches!(err, PresetError::MissingName), "{err}");
        let err = store.autosave(&preset).unwrap_err();
        assert!(matches!(err, PresetError::MissingName), "{err}");

        let written = std::fs::read_dir(tmp.join("user")).map_or(0, Iterator::count);
        assert_eq!(written, 0, "nothing was written");
    }

    #[test]
    fn autosave_marks_a_preset_modified_and_is_preferred_on_load() {
        let tmp = tempdir("autosave");
        let mut store = store_in(&tmp);

        let (mut jazz, was_autosaved) = store.load("Jazz").expect("load");
        assert!(!was_autosaved);
        assert!(!store.find("Jazz").unwrap().modified);

        jazz.set_effect(fxsound_core::Effect::Ambience, 0.75);
        store.autosave(&jazz).expect("autosave");
        store.rescan();
        assert!(store.find("Jazz").unwrap().modified);

        let (loaded, from_autosave) = store.load("Jazz").expect("load autosaved");
        assert!(from_autosave);
        assert!((loaded.effect(fxsound_core::Effect::Ambience) - 0.75).abs() < 0.01);

        store.clear_autosave("Jazz");
        assert!(!store.find("Jazz").unwrap().modified);
        let (_, from_autosave) = store.load("Jazz").expect("load after clear");
        assert!(!from_autosave);
    }

    #[test]
    fn an_export_writes_the_saved_preset_and_never_its_unsaved_edits() {
        // The original's PR #155: the export went through the autosave-preferring load, so a preset
        // with unsaved edits was handed on under its name carrying edits nobody had saved.
        let tmp = tempdir("export-saved");
        let mut store = store_in(&tmp);
        let (mut jazz, _) = store.load("Jazz").expect("load");
        let saved = jazz.effect(fxsound_core::Effect::Ambience);
        jazz.set_effect(fxsound_core::Effect::Ambience, 0.75);
        store.autosave(&jazz).expect("autosave");
        store.rescan();
        assert!(store.find("Jazz").unwrap().modified);

        let out = tmp.join("export");
        std::fs::create_dir_all(&out).expect("mkdir");
        let path = store.export("Jazz", &out).expect("export");
        let exported = crate::load(&path).expect("parse export");
        assert_eq!(exported.name, "Jazz");
        assert!(
            (exported.effect(fxsound_core::Effect::Ambience) - saved).abs() < 0.01,
            "the saved value, not the edit"
        );
        assert!(
            store.find("Jazz").unwrap().modified,
            "and the edits are still there to save"
        );
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            PresetStore::file_name("Jazz").ok().as_deref(),
            "where the store says it files the name"
        );
    }

    #[test]
    fn loading_as_saved_reads_the_presets_file_past_its_autosave() {
        let tmp = tempdir("load-saved");
        let store = store_in(&tmp);
        let (mut jazz, _) = store.load("Jazz").expect("load");
        let saved = jazz.effect(fxsound_core::Effect::Ambience);
        jazz.set_effect(fxsound_core::Effect::Ambience, 0.75);
        store.autosave(&jazz).expect("autosave");

        let (stashed, from_autosave) = store.load("Jazz").expect("load");
        assert!(from_autosave);
        assert!((stashed.effect(fxsound_core::Effect::Ambience) - 0.75).abs() < 0.01);
        let as_saved = store.load_saved("Jazz").expect("load as saved");
        assert_eq!(as_saved.name, "Jazz");
        assert!((as_saved.effect(fxsound_core::Effect::Ambience) - saved).abs() < 0.01);
        assert!(store.load_saved("Nope").is_err(), "an unknown name");
    }

    #[test]
    fn factory_presets_cannot_be_deleted() {
        let tmp = tempdir("delete");
        let mut store = store_in(&tmp);
        let err = store.delete("Jazz").unwrap_err();
        assert!(matches!(err, PresetError::Io(_)));
        assert!(store.find("Jazz").is_some());
    }

    #[test]
    fn import_round_trips_through_the_user_directory() {
        let tmp = tempdir("import");
        let mut store = store_in(&tmp);
        let source = assets().join("BonusPresets/Metal.fac");

        let name = store.import(&source).expect("import");
        assert_eq!(name, "Metal");
        assert_eq!(store.find("Metal").unwrap().source, PresetSource::User);

        let exported_dir = tmp.join("export");
        std::fs::create_dir_all(&exported_dir).expect("mkdir");
        let path = store.export("Metal", &exported_dir).expect("export");
        assert!(path.is_file());
        let (original, _) = store.load("Metal").expect("load");
        assert_eq!(crate::load(&path).expect("parse export"), original);
    }

    #[test]
    fn overwriting_a_user_preset_keeps_the_previous_version_without_listing_it() {
        let tmp = tempdir("overwrite-backup");
        let mut store = PresetStore::with_dirs(Vec::new(), tmp.join("user"));

        let mut preset = Preset {
            name: "Mine".into(),
            ..Preset::default()
        };
        preset.main_midi[0] = 10;
        store.save_as(&preset, "Mine").expect("first save");

        preset.main_midi[0] = 99;
        let path = store.save_as(&preset, "Mine").expect("overwrite");

        let mut backup = path.as_os_str().to_owned();
        backup.push(".bak");
        let kept = crate::load(std::path::Path::new(&backup)).expect("the backup is a valid .fac");
        assert_eq!(
            kept.main_midi[0], 10,
            "the backup should hold the old value"
        );
        assert_eq!(
            crate::load(&path).expect("load").main_midi[0],
            99,
            "the live file should hold the new one"
        );

        // A `.bak` beside a preset must not become a second entry in the list.
        let names: Vec<_> = store.entries().iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["Mine".to_owned()], "listed: {names:?}");
    }

    #[test]
    fn a_name_that_would_take_another_presets_file_is_refused() {
        // `Mu:sic` and `Music` are two names and one file: the sanitiser strips the `:` and both
        // become `Music.fac`. Beside the factory Music the two would share an autosave slot;
        // beside a user Music the save would replace its file — with a `.bak`, but the list
        // would then show one name where two had been saved. Neither is a save, so neither
        // happens.
        let tmp = tempdir("shared-file");
        let mut store = store_in(&tmp);
        assert_eq!(
            store.find("Music").map(|e| e.source),
            Some(PresetSource::Factory)
        );

        let music = Preset {
            name: "Mu:sic".into(),
            ..Preset::default()
        };
        let err = store.save_as(&music, "Mu:sic").unwrap_err();
        let PresetError::SharedFile {
            name,
            existing,
            file,
        } = &err
        else {
            panic!("expected SharedFile, got {err}");
        };
        assert_eq!(name, "Mu:sic");
        assert_eq!(existing, "Music");
        assert_eq!(file, "Music.fac");
        assert!(store.find("Mu:sic").is_none());
        assert!(!tmp.join("Music.fac").exists(), "nothing was written");

        // Between two user presets the file really is the same one.
        let mut mine = Preset {
            name: "Mine".into(),
            ..Preset::default()
        };
        mine.main_midi[0] = 10;
        let path = store.save_as(&mine, "Mine").expect("save Mine");
        let err = store.save_as(&mine, "Mi?ne").unwrap_err();
        assert!(
            matches!(&err, PresetError::SharedFile { existing, .. } if existing == "Mine"),
            "{err}"
        );
        assert!(store.find("Mi?ne").is_none());
        assert_eq!(crate::load(&path).expect("Mine is intact").main_midi[0], 10);
        let mut backup = path.as_os_str().to_owned();
        backup.push(".bak");
        assert!(
            !Path::new(&backup).exists(),
            "a refused save leaves no backup either"
        );

        // The same name is the overwrite, and still goes through with its `.bak`.
        mine.main_midi[0] = 99;
        store.save_as(&mine, "Mine").expect("overwrite Mine");
        assert!(Path::new(&backup).is_file());
        assert_eq!(crate::load(&path).expect("reload").main_midi[0], 99);
    }

    #[test]
    fn a_save_or_rename_onto_the_file_another_preset_is_listed_from_is_refused() {
        // A hand-copied `Rock.fac` that calls itself "Rock Concert" is listed under that name,
        // while `Rock.fac` is the file the name "Rock" maps to. Saving "Rock" replaced it and
        // renaming a preset to "Rock" set it aside: either way "Rock Concert" left the list, kept
        // only in a `.bak` the list does not show.
        let tmp = tempdir("listed-from");
        let user = tmp.join("user");
        preset_at(&user.join("Rock.fac"), "Rock Concert", 11);
        let mut store = PresetStore::with_dirs(Vec::new(), user.clone());
        store.rescan();
        let mut mine = Preset {
            name: "Mine".into(),
            ..Preset::default()
        };
        mine.main_midi[0] = 22;
        store.save_as(&mine, "Mine").expect("save Mine");

        let err = store.save_as(&mine, "Rock").unwrap_err();
        assert!(
            matches!(&err, PresetError::SharedFile { existing, file, .. }
                if existing == "Rock Concert" && file == "Rock.fac"),
            "{err}"
        );
        let err = store.rename("Mine", "Rock").unwrap_err();
        assert!(
            matches!(&err, PresetError::SharedFile { existing, .. } if existing == "Rock Concert"),
            "{err}"
        );

        let kept = crate::load(&user.join("Rock.fac")).expect("Rock Concert's file");
        assert_eq!(
            (kept.name.as_str(), kept.main_midi[0]),
            ("Rock Concert", 11)
        );
        let mut names: Vec<_> = store.entries().iter().map(|e| e.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["Mine", "Rock Concert"]);
        assert!(
            !user.join("Rock.fac.bak").exists(),
            "nothing was overwritten"
        );
        assert!(
            !user.join("Rock.fac.1.bak").exists(),
            "nothing was set aside"
        );
        assert_eq!(store.load("Mine").expect("Mine").0.main_midi[0], 22);
    }

    #[cfg(windows)]
    #[test]
    fn on_windows_a_name_that_differs_from_a_listed_one_only_in_case_is_its_file() {
        // NTFS folds case: `JAZZ.fac` is `Jazz.fac`. A user preset "JAZZ" beside the factory Jazz,
        // put there by hand or brought over from Linux, is replaced by the user's own copy of the
        // factory Jazz, which `save_as` writes to `Jazz.fac`.
        let tmp = tempdir("case-folds");
        let mut store = store_in(&tmp);
        preset_at(&tmp.join("JAZZ.fac"), "JAZZ", 11);
        store.rescan();
        let (jazz, _) = store.load("Jazz").expect("the factory Jazz");
        let err = store.save_as(&jazz, "Jazz").unwrap_err();
        assert!(
            matches!(&err, PresetError::SharedFile { existing, .. } if existing == "JAZZ"),
            "{err}"
        );
        let kept = crate::load(&tmp.join("JAZZ.fac")).expect("JAZZ's file");
        assert_eq!((kept.name.as_str(), kept.main_midi[0]), ("JAZZ", 11));

        // A new name that differs from the factory Music only in case has no file to take yet, but
        // it would share Music's autosave and export: `MUSIC.fac` is `Music.fac` there too.
        let err = store.save_as(&jazz, "MUSIC").unwrap_err();
        assert!(
            matches!(&err, PresetError::SharedFile { existing, .. } if existing == "Music"),
            "{err}"
        );
        assert!(store.find("MUSIC").is_none());

        // A rename onto another preset's name in another case is refused; onto its own, allowed.
        let err = store.rename("JAZZ", "music").unwrap_err();
        assert!(
            matches!(&err, PresetError::SharedFile { existing, .. } if existing == "Music"),
            "{err}"
        );
        store.rename("JAZZ", "Jazz Two").expect("a name of its own");
        store
            .rename("Jazz Two", "JAZZ TWO")
            .expect("its own name in another case");
        assert!(store.find("JAZZ TWO").is_some());
    }

    #[cfg(windows)]
    #[test]
    fn on_windows_a_letter_ntfs_does_not_fold_is_a_name_of_its_own() {
        // Unicode uppercases the dotless `ı` to `I`, which lowercases to `i`, and NTFS keeps
        // `Kayıt.fac` and `Kayit.fac` apart; so it keeps the micro sign from the Greek `μ`, the
        // long `ſ` from `s` and the final `ς` from `σ`. Folding every uppercase refused the second
        // name of each pair, which the controller lets through, and a rename onto it, although
        // each is a file of its own. The voice store is the same `Store`.
        fn save(store: &mut PresetStore, name: &str, main0: u8) {
            let mut preset = Preset {
                name: name.into(),
                ..Preset::default()
            };
            preset.main_midi[0] = main0;
            if let Err(err) = store.save_as(&preset, name) {
                panic!("save {name}: {err}");
            }
        }
        let tmp = tempdir("letters-apart");
        let user = tmp.join("user");
        let mut store = PresetStore::with_dirs(Vec::new(), user.clone());
        let dotless = "Kay\u{131}t";
        save(&mut store, dotless, 11);
        save(&mut store, "Kayit", 12);
        store
            .rename("Kayit", "Elsewhere")
            .expect("rename Kayit away");
        store
            .rename("Elsewhere", "Kayit")
            .expect("and back, beside Kayıt");
        let pairs = [
            // The micro sign and the Greek mu, the long s and the s, the final sigma and the sigma.
            ("\u{B5}-Bass", "\u{3BC}-Bass"),
            ("Ga\u{17F}t", "Gast"),
            ("Λόγο\u{3C2}", "Λόγο\u{3C3}"),
        ];
        let mut expected = vec![(dotless, 11), ("Kayit", 12)];
        for ((first, second), main0) in pairs.into_iter().zip((13..).step_by(2)) {
            save(&mut store, first, main0);
            save(&mut store, second, main0 + 1);
            expected.extend([(first, main0), (second, main0 + 1)]);
        }

        for (name, main0) in &expected {
            let (preset, _) = store.load(name).expect("listed");
            assert_eq!(preset.main_midi[0], *main0, "{name} holds its own values");
        }
        let files = std::fs::read_dir(&user)
            .expect("the user directory")
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|x| x == "fac"))
            .count();
        assert_eq!(files, expected.len(), "a file each");
        assert_eq!(store.entries().len(), expected.len(), "each listed once");

        // The letters NTFS does fold are still one file: `KAYIT` and `kayit` are `Kayit`.
        let err = store.save_as(&Preset::default(), "KAYIT").unwrap_err();
        assert!(
            matches!(&err, PresetError::SharedFile { existing, .. } if existing == "Kayit"),
            "{err}"
        );
        let err = store.rename(dotless, "kayit").unwrap_err();
        assert!(
            matches!(&err, PresetError::SharedFile { existing, .. } if existing == "Kayit"),
            "{err}"
        );
        assert_eq!(store.load("Kayit").expect("Kayit").0.main_midi[0], 12);
        assert_eq!(store.load(dotless).expect("Kayıt").0.main_midi[0], 11);
    }

    #[cfg(windows)]
    #[test]
    fn on_windows_same_file_name_makes_two_names_one_file_where_ntfs_does() {
        // Each pair of letters goes into a file name, and the volume is asked whether the other
        // name finds the file. Wherever NTFS makes the two names one file `same_file_name` must
        // too, or a save could take another preset's autosave and export; and the letters it keeps
        // apart, which a Turkish, German or Greek name holds, must stay apart. The answers are
        // those of the table Windows 7 through 11 write: the case pairs newer than it — Georgian's
        // capitals and the like — are folded on purpose (`upcase`) and not asked about, and a
        // volume formatted by XP or Vista, whose older table also makes the final `ς` one with `Σ`
        // and `σ`, is skipped with a note, as a folder that tells case apart is. The store asks
        // such a volume itself (`alias_of`).
        let tmp = tempdir("upcase-volume");
        let one_file = |a: char, b: char| {
            let file = tmp.join(format!("x{a}y.fac"));
            std::fs::write(&file, "").expect("write the probe");
            let found = tmp.join(format!("x{b}y.fac")).exists();
            std::fs::remove_file(&file).expect("remove the probe");
            found
        };
        if !one_file('a', 'A') {
            eprintln!("{}: this folder tells case apart; skipped", tmp.display());
            return;
        }
        if one_file('\u{3C2}', '\u{3A3}') {
            eprintln!(
                "{}: this volume's case table is older than Windows 7's; skipped",
                tmp.display()
            );
            return;
        }
        let pairs = [
            // One file: plain case pairs, and the Greek letters with iota subscript and their
            // capitals, which the standard library uppercases to two letters.
            ('a', 'A'),
            ('é', 'É'),
            ('ÿ', 'Ÿ'),
            ('я', 'Я'),
            ('ё', 'Ё'),
            ('σ', 'Σ'),
            ('ό', 'Ό'),
            ('\u{1C6}', '\u{1C4}'),
            ('\u{1F80}', '\u{1F88}'),
            ('\u{1FB3}', '\u{1FBC}'),
            ('\u{1FF3}', '\u{1FFC}'),
            // Two: the dotless and the dotted i, the long s, the micro sign, the final sigma, the
            // titlecase dž, the curled beta, the Kelvin sign, the sharp s and its capital, and a
            // Deseret letter, beyond the Basic Multilingual Plane, with its capital.
            ('\u{131}', 'I'),
            ('\u{131}', 'i'),
            ('\u{130}', 'i'),
            ('\u{17F}', 'S'),
            ('\u{17F}', 's'),
            ('\u{B5}', '\u{39C}'),
            ('\u{B5}', '\u{3BC}'),
            ('\u{3C2}', '\u{3A3}'),
            ('\u{3C2}', '\u{3C3}'),
            ('\u{1C5}', '\u{1C4}'),
            ('\u{1C5}', '\u{1C6}'),
            ('\u{3D0}', '\u{3B2}'),
            ('\u{212A}', 'k'),
            ('\u{DF}', '\u{1E9E}'),
            ('\u{10428}', '\u{10400}'),
        ];
        for (a, b) in pairs {
            assert_eq!(
                same_file_name(&format!("x{a}y.fac"), &format!("x{b}y.fac")),
                one_file(a, b),
                "U+{:04X} and U+{:04X}: same_file_name, then the volume",
                u32::from(a),
                u32::from(b)
            );
        }
    }

    #[cfg(not(unix))]
    #[test]
    fn upcase_pairs_a_letter_only_with_a_capital_that_lowercases_back_to_it() {
        // Over every character: the capital a letter is upcased to lowercases back to it, is
        // upcased to itself and is one UTF-16 unit, as the letter is. And each capital whose
        // uppercase is two letters, as its small letter's is — `ᾼ` and `ᾳ`, both `ΑΙ` — is that
        // small letter's capital, which only the hand-made match makes it.
        let mut through_the_lowercase = 0;
        for c in (0..=u32::from(char::MAX)).filter_map(char::from_u32) {
            let at = u32::from(c);
            let up = upcase(c);
            assert_eq!(upcase(up), up, "U+{at:04X}");
            if up != c {
                assert!(up.to_lowercase().eq([c]), "U+{at:04X}");
                assert_eq!((c.len_utf16(), up.len_utf16()), (1, 1), "U+{at:04X}");
            }
            let mut lower = c.to_lowercase();
            if let (Some(small), None) = (lower.next(), lower.next())
                && small != c
                && c.to_uppercase().len() > 1
                && small.to_uppercase().eq(c.to_uppercase())
            {
                assert_eq!(upcase(small), c, "U+{at:04X}");
                through_the_lowercase += 1;
            }
        }
        assert_eq!(
            through_the_lowercase, 27,
            "the capitals with prosgegrammeni"
        );
    }

    /// A preset called `name` whose first main value is `main0`, written nowhere yet.
    #[cfg(windows)]
    fn valued(name: &str, main0: u8) -> Preset {
        let mut preset = Preset {
            name: name.into(),
            ..Preset::default()
        };
        preset.main_midi[0] = main0;
        preset
    }

    /// The `.fac` files in `dir`, by name.
    #[cfg(windows)]
    fn fac_files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("read the folder")
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|x| x == "fac"))
            .filter_map(|path| path.file_name()?.to_str().map(str::to_owned))
            .collect();
        names.sort_unstable();
        names
    }

    /// Whether `tmp`'s volume gives a long name an 8.3 short name: `Rock Concert Live.fac`, made
    /// in a folder of its own, found again as `ROCKCO~1.fac`. Windows can turn them off for each
    /// volume, and a test that needs them then says so and passes.
    #[cfg(windows)]
    fn makes_short_names(tmp: &Path) -> bool {
        let probe = tmp.join("short-name-probe");
        std::fs::create_dir_all(&probe).expect("make the probe's folder");
        std::fs::write(probe.join("Rock Concert Live.fac"), "").expect("write the probe");
        let made = probe.join("ROCKCO~1.fac").exists();
        std::fs::remove_dir_all(&probe).expect("remove the probe");
        if !made {
            eprintln!(
                "{}: this volume makes no 8.3 short names; skipped",
                tmp.display()
            );
        }
        made
    }

    #[cfg(windows)]
    #[test]
    fn on_windows_a_name_that_is_another_files_short_name_never_takes_its_place() {
        // NTFS gives `Rock Concert Live.fac` the 8.3 short name `ROCKCO~1.FAC` in a folder with no
        // `ROCKCO~1.fac` in it yet, and the name `ROCKCO~1.fac` then opens that file. In `AutoSave`
        // that made Rock Concert Live's unsaved edits those of a preset called `ROCKCO~1`: listed
        // with its `*` and loaded in place of its own values, then replaced by its own next stash,
        // which left Rock Concert Live as last saved. Its undo, save or delete dropped them too.
        fn save(store: &mut PresetStore, name: &str, main0: u8) {
            if let Err(err) = store.save_as(&valued(name, main0), name) {
                panic!("save {name}: {err}");
            }
        }
        fn kept(store: &PresetStore, after: &str) {
            let entry = store.find("Rock Concert Live").expect("listed");
            assert!(
                entry.modified,
                "after {after}: Rock Concert Live keeps its *"
            );
            let (loaded, from_autosave) = store.load("Rock Concert Live").expect("load");
            assert_eq!(
                (loaded.main_midi[0], from_autosave),
                (55, true),
                "after {after}: its unsaved edits"
            );
        }
        let tmp = tempdir("short-names");
        if !makes_short_names(&tmp) {
            return;
        }
        let user = tmp.join("user");
        let mut store = PresetStore::with_dirs(Vec::new(), user.clone());
        save(&mut store, "ROCKCO~1", 1);
        save(&mut store, "Rock Concert Live", 2);
        // Second in the user folder, its file is `ROCKCO~2` there; first in `AutoSave`, its
        // unsaved edits are `ROCKCO~1`.
        assert!(user.join("ROCKCO~2.fac").exists(), "its file's short name");
        store
            .autosave(&valued("Rock Concert Live", 55))
            .expect("its unsaved edits");
        assert!(
            user.join("AutoSave/ROCKCO~1.fac").exists(),
            "its autosave's short name"
        );
        store.rescan();
        kept(&store, "the listing");

        // They are not ROCKCO~1's.
        assert!(!store.find("ROCKCO~1").expect("listed").modified);
        let (own, from_autosave) = store.load("ROCKCO~1").expect("load ROCKCO~1");
        assert_eq!((own.main_midi[0], from_autosave), (1, false));

        // Its own stash, which would replace them, is refused as a file of Rock Concert Live's.
        let err = store.autosave(&valued("ROCKCO~1", 66)).unwrap_err();
        assert!(
            matches!(&err, PresetError::SharedFile { name, existing, file }
                if name == "ROCKCO~1" && existing == "Rock Concert Live" && file == "ROCKCO~1.fac"),
            "{err}"
        );
        kept(&store, "its stash");

        // Its undo, its save and its delete leave them alone.
        store.clear_autosave("ROCKCO~1");
        kept(&store, "its undo");
        save(&mut store, "ROCKCO~1", 3);
        kept(&store, "its save");
        store
            .delete("ROCKCO~1")
            .expect("delete")
            .expect("it was listed");
        kept(&store, "its delete");

        // A rename onto the name that brings unsaved edits would put them in their place: refused
        // before anything moves. Without any, it goes through and they stay.
        save(&mut store, "Other", 4);
        store
            .autosave(&valued("Other", 44))
            .expect("Other's unsaved edits");
        store.rescan();
        let err = store.rename("Other", "ROCKCO~1").unwrap_err();
        assert!(
            matches!(&err, PresetError::SharedFile { existing, .. } if existing == "Rock Concert Live"),
            "{err}"
        );
        let (other, from_autosave) = store.load("Other").expect("Other is still there");
        assert_eq!((other.main_midi[0], from_autosave), (44, true));
        kept(&store, "a refused rename");
        store.clear_autosave("Other");
        store
            .rename("Other", "ROCKCO~1")
            .expect("a rename with nothing unsaved");
        assert!(!store.find("ROCKCO~1").expect("renamed").modified);
        assert_eq!(store.load("ROCKCO~1").expect("load").0.main_midi[0], 4);
        kept(&store, "a rename");

        // The short name of Rock Concert Live's own file is still its file, as `listed_from` finds.
        let err = store
            .save_as(&valued("ROCKCO~2", 5), "ROCKCO~2")
            .unwrap_err();
        assert!(
            matches!(&err, PresetError::SharedFile { existing, .. } if existing == "Rock Concert Live"),
            "{err}"
        );

        // A file the list does not show is neither saved over nor set aside under its short name.
        std::fs::write(user.join("Loose Concert.fac"), "not a preset").expect("write");
        store.rescan();
        let err = store
            .save_as(&valued("LOOSEC~1", 6), "LOOSEC~1")
            .unwrap_err();
        assert!(
            matches!(&err, PresetError::SharedFile { existing, .. } if existing == "Loose Concert.fac"),
            "{err}"
        );
        let err = store.rename("ROCKCO~1", "LOOSEC~1").unwrap_err();
        assert!(matches!(err, PresetError::SharedFile { .. }), "{err}");
        assert_eq!(
            std::fs::read_to_string(user.join("Loose Concert.fac")).expect("the loose file"),
            "not a preset"
        );

        // Nor does an export of ROCKCO~1 replace Rock Concert Live's, whose short name it is there.
        let out = tmp.join("out");
        store
            .export("Rock Concert Live", &out)
            .expect("export Rock Concert Live");
        let err = store.export("ROCKCO~1", &out).unwrap_err();
        assert!(
            matches!(&err, PresetError::SharedFile { existing, .. } if existing == "Rock Concert Live"),
            "{err}"
        );
        assert_eq!(fac_files(&out), ["Rock Concert Live.fac"]);
        assert_eq!(
            crate::load(&out.join("Rock Concert Live.fac"))
                .expect("its export")
                .main_midi[0],
            2
        );

        // Renamed away, ROCKCO~1 takes only what is its own: no unsaved edits.
        store.rename("ROCKCO~1", "Elsewhere").expect("rename away");
        let (moved, from_autosave) = store.load("Elsewhere").expect("renamed");
        assert_eq!((moved.main_midi[0], from_autosave), (4, false));
        kept(&store, "a rename away");

        assert_eq!(fac_files(&user.join("AutoSave")), ["Rock Concert Live.fac"]);
        assert_eq!(
            fac_files(&user),
            [
                "Elsewhere.fac",
                "Loose Concert.fac",
                "Rock Concert Live.fac"
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn on_windows_a_preset_renamed_to_its_own_short_name_takes_its_file_and_edits_along() {
        // `ROCKCO~1.fac` is Rock Concert Live's own file, and in `AutoSave` its own unsaved edits:
        // nobody else's to keep apart from, so a rename to that name moves both.
        let tmp = tempdir("own-short-name");
        if !makes_short_names(&tmp) {
            return;
        }
        let user = tmp.join("user");
        let mut store = PresetStore::with_dirs(Vec::new(), user.clone());
        store
            .save_as(&valued("Rock Concert Live", 2), "Rock Concert Live")
            .expect("save");
        store
            .autosave(&valued("Rock Concert Live", 55))
            .expect("its unsaved edits");
        assert!(user.join("ROCKCO~1.fac").exists() && user.join("AutoSave/ROCKCO~1.fac").exists());
        store.rescan();

        store
            .rename("Rock Concert Live", "ROCKCO~1")
            .expect("its own short name");
        assert!(
            store.find("ROCKCO~1").expect("listed").modified,
            "with its *"
        );
        let (loaded, from_autosave) = store.load("ROCKCO~1").expect("load");
        assert_eq!(
            (loaded.name.as_str(), loaded.main_midi[0], from_autosave),
            ("ROCKCO~1", 55, true)
        );
        assert_eq!(
            store.load_saved("ROCKCO~1").expect("as saved").main_midi[0],
            2
        );
        assert_eq!(store.entries().len(), 1);
        assert_eq!(fac_files(&user), ["ROCKCO~1.fac"]);
        assert_eq!(fac_files(&user.join("AutoSave")), ["ROCKCO~1.fac"]);
    }

    #[cfg(windows)]
    #[test]
    fn on_windows_a_link_under_a_presets_name_is_its_own_whatever_its_file_is_called() {
        // `canonicalize` names the file at the end of a link, which is not the name the link is
        // under: unsaved edits kept as a link to a file of another name, as a dotfile manager
        // keeps one, are still the preset's own, read and stashed through the link.
        /// `ERROR_PRIVILEGE_NOT_HELD`: no `SeCreateSymbolicLinkPrivilege`.
        const PRIVILEGE_NOT_HELD: i32 = 1314;
        let tmp = tempdir("linked-autosave");
        let user = tmp.join("user");
        let mut store = PresetStore::with_dirs(Vec::new(), user.clone());
        store.save_as(&valued("Mine", 1), "Mine").expect("save");
        let stash = tmp.join("dotfiles/stash.fac");
        preset_at(&stash, "Mine", 77);
        std::fs::create_dir_all(user.join("AutoSave")).expect("mkdir");
        match std::os::windows::fs::symlink_file(&stash, user.join("AutoSave/Mine.fac")) {
            Ok(()) => {}
            Err(err) if err.raw_os_error() == Some(PRIVILEGE_NOT_HELD) => {
                eprintln!("skipped: this process may not make symbolic links ({err})");
                return;
            }
            Err(err) => panic!("a symbolic link: {err}"),
        }
        store.rescan();

        assert!(store.find("Mine").expect("listed").modified, "with its *");
        let (loaded, from_autosave) = store.load("Mine").expect("load");
        assert_eq!((loaded.main_midi[0], from_autosave), (77, true));
        store
            .autosave(&valued("Mine", 78))
            .expect("stashed through the link");
        assert_eq!(
            crate::load(&stash).expect("the linked file").main_midi[0],
            78
        );
        store.clear_autosave("Mine");
        assert!(!user.join("AutoSave/Mine.fac").exists(), "undone");
        assert!(stash.is_file(), "the link went, not the file it leads to");
    }

    /// A preset called `name` whose first main value is `main0`, written to `path` as it is.
    fn preset_at(path: &Path, name: &str, main0: u8) -> Preset {
        let mut preset = Preset {
            name: name.into(),
            ..Preset::default()
        };
        preset.main_midi[0] = main0;
        crate::save(&preset, path).expect("write the preset");
        preset
    }

    /// The user directory as 0.3.0 left a preset saved from its window: the file under the name
    /// as written, `Rock:Live.fac`, and its unsaved edits under the same name in `AutoSave/`.
    #[cfg(unix)]
    fn as_0_3_0_left_it(tag: &str) -> (ScratchDir, PresetStore) {
        let tmp = tempdir(tag);
        let user = tmp.join("user");
        preset_at(&user.join("Rock:Live.fac"), "Rock:Live", 11);
        preset_at(&user.join("AutoSave/Rock:Live.fac"), "Rock:Live", 77);
        let mut store = PresetStore::with_dirs(Vec::new(), user).with_trash(tmp.join("Trash"));
        store.rescan();
        (tmp, store)
    }

    // Unix only: 0.3.0 ran on Linux alone, and Windows cannot name a file `Rock:Live.fac`.
    #[cfg(unix)]
    #[test]
    fn the_unsaved_edits_0_3_0_kept_under_a_name_as_written_are_found() {
        let (_tmp, store) = as_0_3_0_left_it("legacy-autosave-found");
        assert!(
            store.find("Rock:Live").expect("listed").modified,
            "with its *"
        );
        let (loaded, from_autosave) = store.load("Rock:Live").expect("load");
        assert!(from_autosave, "the edits, not the file as last saved");
        assert_eq!(loaded.main_midi[0], 77);
    }

    // Unix only: 0.3.0 ran on Linux alone, and Windows cannot name a file `Rock:Live.fac`.
    #[cfg(unix)]
    #[test]
    fn a_user_preset_is_saved_over_the_file_it_is_listed_from_whatever_that_is_called() {
        // 0.3.0 filed "Rock:Live" as `Rock:Live.fac`. The save went to `RockLive.fac`, the list
        // stayed on the old file — `:` sorts before letters — and the save looked lost, while
        // the autosave holding the same edits had already been cleared.
        let (tmp, mut store) = as_0_3_0_left_it("legacy-save");
        let user = tmp.join("user");
        let (mut edited, _) = store.load("Rock:Live").expect("load");
        edited.main_midi[0] = 99;

        let path = store.save_as(&edited, "Rock:Live").expect("save");

        assert_eq!(path, user.join("Rock:Live.fac"));
        assert!(!user.join("RockLive.fac").exists(), "no second file");
        let (loaded, from_autosave) = store.load("Rock:Live").expect("load");
        assert!(!from_autosave);
        assert_eq!(loaded.main_midi[0], 99, "what was saved is what loads");
        assert_eq!(
            crate::load(&user.join("Rock:Live.fac.bak"))
                .expect("the overwrite's backup")
                .main_midi[0],
            11
        );
        assert!(
            !user.join("AutoSave/Rock:Live.fac").exists(),
            "the save took in the edits, so they are not a change any more"
        );
        assert!(!store.find("Rock:Live").expect("listed").modified);
        assert_eq!(store.entries().len(), 1);
    }

    #[test]
    fn a_hand_copied_file_whose_stem_is_not_its_name_is_saved_over_in_place() {
        let tmp = tempdir("hand-copied");
        let user = tmp.join("user");
        preset_at(&user.join("from a forum.fac"), "Evening Jazz", 5);
        let mut store = PresetStore::with_dirs(Vec::new(), user.clone());
        store.rescan();
        let (mut edited, _) = store.load("Evening Jazz").expect("load");
        edited.main_midi[0] = 6;

        let path = store.save_as(&edited, "Evening Jazz").expect("save");

        assert_eq!(path, user.join("from a forum.fac"));
        assert!(!user.join("Evening Jazz.fac").exists());
        assert_eq!(store.load("Evening Jazz").expect("load").0.main_midi[0], 6);
    }

    /// `path` as a name a hand-copied file could carry that a path join reads as rooted: on
    /// Windows without its drive, `\Users\…`, which `join` puts on the joined directory's own
    /// drive; elsewhere as it is, `/tmp/…`.
    fn rooted_name(path: &Path) -> String {
        let mut components = path.components();
        if let Some(std::path::Component::Prefix(_)) = components.clone().next() {
            components.next();
        }
        components
            .as_path()
            .to_str()
            .expect("a scratch path in UTF-8")
            .to_owned()
    }

    #[test]
    fn a_name_that_is_a_path_never_reaches_a_file_but_its_own() {
        // A preset is listed by the name its file carries, and a hand-copied file can carry any:
        // on Windows `..\`, `x\..\` and a leading `\` are path syntax. Joined onto `AutoSave/` as
        // the name of an autosave 0.3.0 left, such a name reached out of it — to the user's own
        // preset beside it, to another preset's unsaved edits, anywhere on the drive — and the
        // file found there was the preset's unsaved edits: listed with a `*`, removed for good by
        // the next autosave or undo, sent to the trash by a delete, moved away by a rename.
        //
        // Up out of `AutoSave/`, to the user's own Rock beside it.
        a_path_name_reaches_nothing("parent", |_| r"..\Rock".to_owned());
        // Into `AutoSave/` and back out of a directory that is not there, to Rock's own edits.
        a_path_name_reaches_nothing("through", |_| r"x\..\Rock".to_owned());
        // From the top of the drive, to anywhere on it.
        a_path_name_reaches_nothing("rooted", |outside| rooted_name(&outside.join("Rock")));
        // The same with the separator Unix has too, refused there from the start.
        a_path_name_reaches_nothing("parent-slash", |_| "../Rock".to_owned());
        a_path_name_reaches_nothing("through-slash", |_| "x/../Rock".to_owned());
    }

    /// A user directory holding the preset Rock and its unsaved edits, a file of the same name
    /// outside it, and a hand-copied preset whose name, `name(outside)`, is a path to one of the
    /// three; then everything the store does to that preset, after each of which the three are
    /// as they were.
    fn a_path_name_reaches_nothing(tag: &str, name: impl Fn(&Path) -> String) {
        let tmp = tempdir(&format!("path-name-{tag}"));
        let user = tmp.join("user");
        let outside = tmp.join("outside");
        let name = name(&outside);
        preset_at(&user.join("Rock.fac"), "Rock", 11);
        preset_at(&user.join("AutoSave/Rock.fac"), "Rock", 55);
        preset_at(&outside.join("Rock.fac"), "Rock", 33);
        preset_at(&user.join("from a forum.fac"), &name, 99);
        let mut store =
            PresetStore::with_dirs(Vec::new(), user.clone()).with_trash(tmp.join("Trash"));
        store.rescan();
        let untouched = |after: &str| {
            for (file, main0) in [
                (user.join("Rock.fac"), 11),
                (user.join("AutoSave/Rock.fac"), 55),
                (outside.join("Rock.fac"), 33),
            ] {
                assert_eq!(
                    crate::load(&file).ok().map(|preset| preset.main_midi[0]),
                    Some(main0),
                    "{name:?}, after {after}: {}",
                    file.display()
                );
            }
        };

        assert!(
            !store.find(&name).expect("listed").modified,
            "{name:?}: no unsaved edits of its own"
        );
        let (mut preset, from_autosave) = store.load(&name).expect("load");
        assert!(!from_autosave, "{name:?}");
        assert_eq!(preset.main_midi[0], 99, "{name:?}: its own values");
        untouched("the listing");

        preset.main_midi[0] = 98;
        store.autosave(&preset).expect("autosave");
        untouched("an autosave");
        store.rescan();
        let (edited, from_autosave) = store.load(&name).expect("load the edits");
        assert!(from_autosave, "{name:?}");
        assert_eq!(edited.main_midi[0], 98, "{name:?}: its own edits");

        store.clear_autosave(&name);
        untouched("an undo");
        store.rename(&name, "Renamed").expect("rename away");
        untouched("a rename away");
        store.rename("Renamed", &name).expect("rename back");
        untouched("a rename onto the name");
        store.delete(&name).expect("delete").expect("it was listed");
        untouched("a delete");
    }

    // Unix only: 0.3.0 ran on Linux alone, and Windows cannot name a file `Rock:Live.fac`.
    #[cfg(unix)]
    #[test]
    fn the_next_autosave_moves_0_3_0_edits_to_the_names_own_file() {
        let (tmp, store) = as_0_3_0_left_it("legacy-autosave-moves");
        let autosaves = tmp.join("user/AutoSave");
        let (mut edited, _) = store.load("Rock:Live").expect("load");
        edited.main_midi[0] = 55;

        store.autosave(&edited).expect("autosave");

        assert!(
            !autosaves.join("Rock:Live.fac").exists(),
            "moved, not copied"
        );
        assert_eq!(
            store.autosave_path("Rock:Live"),
            autosaves.join("RockLive.fac")
        );
        let (loaded, from_autosave) = store.load("Rock:Live").expect("load");
        assert!(from_autosave);
        assert_eq!(loaded.main_midi[0], 55);
    }

    // Unix only: 0.3.0 ran on Linux alone, and Windows cannot name a file `Rock:Live.fac`.
    #[cfg(unix)]
    #[test]
    fn undoing_or_deleting_a_preset_takes_its_0_3_0_edits_with_it() {
        let (tmp, mut store) = as_0_3_0_left_it("legacy-autosave-clear");
        let legacy = tmp.join("user/AutoSave/Rock:Live.fac");
        store.clear_autosave("Rock:Live");
        assert!(!legacy.exists(), "undone");
        store.rescan();
        assert!(!store.find("Rock:Live").expect("listed").modified);

        let (tmp, mut store) = as_0_3_0_left_it("legacy-autosave-delete");
        let legacy = tmp.join("user/AutoSave/Rock:Live.fac");
        store
            .delete("Rock:Live")
            .expect("delete")
            .expect("it was listed");
        assert!(!legacy.exists(), "gone from the autosaves");
        assert_eq!(
            crate::load(&tmp.join("Trash/files/Rock:Live.2.fac"))
                .expect("into the trash beside the preset")
                .main_midi[0],
            77
        );
    }

    // Unix only: 0.3.0 ran on Linux alone, and Windows cannot name a file `Rock:Live.fac`.
    #[cfg(unix)]
    #[test]
    fn a_rename_brings_the_0_3_0_edits_along_and_never_takes_over_another_names() {
        let (tmp, mut store) = as_0_3_0_left_it("legacy-autosave-rename");
        let autosaves = tmp.join("user/AutoSave");
        // An orphan 0.3.0 left under a name the rename is about to take.
        preset_at(&autosaves.join("Live?.fac"), "Live?", 33);
        store.rename("Rock:Live", "Live?").expect("rename");

        let (loaded, from_autosave) = store.load("Live?").expect("load");
        assert!(from_autosave, "the preset's own edits came along");
        assert_eq!(loaded.main_midi[0], 77);
        assert!(!autosaves.join("Rock:Live.fac").exists());
        assert!(
            !autosaves.join("Live?.fac").exists(),
            "the orphan went to the trash"
        );

        // Without edits of its own, a leftover under the new name must not become its `*`.
        let (tmp, mut store) = as_0_3_0_left_it("legacy-autosave-rename-leftover");
        let autosaves = tmp.join("user/AutoSave");
        store.clear_autosave("Rock:Live");
        preset_at(&autosaves.join("Live?.fac"), "Live?", 33);
        store.rename("Rock:Live", "Live?").expect("rename");
        assert!(!store.find("Live?").expect("listed").modified);
        assert!(!autosaves.join("Live?.fac").exists(), "into the trash");
    }

    #[test]
    fn a_name_with_a_line_break_is_saved_as_a_preset_the_list_still_shows() {
        // `crossfx --save_preset="$(xclip -o)"` with a multi-line selection: the file was
        // written with the break in its name line, did not parse, and the preset vanished.
        let tmp = tempdir("line-break");
        let mut store = PresetStore::with_dirs(Vec::new(), tmp.join("user"));
        let preset = Preset::default();

        let name = new_preset_name("Line one\nLine two");
        store.save_as(&preset, &name).expect("save");
        store.rescan();
        assert_eq!(
            store.find(&name).map(|entry| entry.name.as_str()),
            Some("Line one Line two")
        );
        assert!(store.load(&name).is_ok());

        // Handed to the store as it is, it is still a file the list reads back.
        let path = store.save_as(&preset, "Carriage\r\nreturn").expect("save");
        store.rescan();
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("Carriage return.fac")
        );
        assert!(store.find("Carriage return").is_some());
        assert_eq!(store.entries().len(), 2);
    }

    fn twenty_band_preset(name: &str, ladder: &[f32; 20]) -> Preset {
        Preset {
            name: name.into(),
            eq_bands: ladder
                .iter()
                .enumerate()
                .map(|(i, &hz)| fxsound_core::EqBand::new(hz, i as f32 - 10.0))
                .collect(),
            ..Preset::default()
        }
    }

    #[test]
    fn a_deleted_preset_goes_to_the_trash_with_a_record_of_where_it_was() {
        // 0.4.0 audit #16: the original deletes a user preset for good.
        let tmp = tempdir("delete-to-trash");
        let trash = tmp.join("Trash");
        let mut store =
            PresetStore::with_dirs(Vec::new(), tmp.join("user")).with_trash(trash.clone());
        let mut mine = Preset {
            name: "Mine".into(),
            ..Preset::default()
        };
        mine.main_midi[0] = 42;
        let path = store.save_as(&mine, "Mine").expect("save");
        mine.main_midi[0] = 43;
        store.autosave(&mine).expect("autosave");

        let went = store
            .delete("Mine")
            .expect("delete")
            .expect("it was listed");
        let crate::trash::Discarded::Trash(trashed) = went else {
            panic!("expected the trash, got {went:?}");
        };
        assert_eq!(trashed, trash.join("files/Mine.fac"));
        assert!(!path.exists(), "gone from the user directory");
        assert!(store.find("Mine").is_none(), "and from the list");
        assert_eq!(
            crate::load(&trashed)
                .expect("the trashed file is the preset")
                .main_midi[0],
            42,
            "the saved preset, restorable"
        );
        let info = std::fs::read_to_string(trash.join("info/Mine.fac.trashinfo")).expect("info");
        assert!(info.contains("Mine.fac"), "{info}");
        assert!(
            !store.autosave_path("Mine").exists(),
            "its unsaved edits went with it"
        );
        // Deleting a name that is not there is nothing, not an error.
        assert_eq!(store.delete("Mine").expect("no-op"), None);
    }

    #[test]
    fn deleting_a_preset_with_unsaved_changes_puts_them_in_the_trash_and_restoring_both_brings_them_back()
     {
        // The edits exist nowhere but in the autosave, and a modified preset can be deleted:
        // dropping the autosave lost them for good however the preset itself was kept.
        let tmp = tempdir("delete-unsaved");
        let trash = tmp.join("Trash");
        let mut store =
            PresetStore::with_dirs(Vec::new(), tmp.join("user")).with_trash(trash.clone());
        let mut mine = Preset {
            name: "Mine".into(),
            ..Preset::default()
        };
        mine.main_midi[0] = 42;
        let path = store.save_as(&mine, "Mine").expect("save");
        mine.main_midi[0] = 43;
        store.autosave(&mine).expect("autosave");
        store.rescan();
        assert!(store.find("Mine").expect("listed").modified);
        let autosave = store.autosave_path("Mine");

        store
            .delete("Mine")
            .expect("delete")
            .expect("it was listed");
        assert!(!autosave.exists(), "gone from the autosaves");
        // The preset took its own name in the trash, so its edits take the next one.
        let edits = trash.join("files/Mine.2.fac");
        assert_eq!(
            crate::load(&edits)
                .expect("the trashed autosave is a preset")
                .main_midi[0],
            43,
            "the unsaved edits, recoverable"
        );
        let info =
            std::fs::read_to_string(trash.join("info/Mine.2.fac.trashinfo")).expect("its record");
        let autosave = std::path::absolute(&autosave).expect("absolute");
        assert!(
            info.lines()
                .any(|line| line == format!("Path={}", crate::trash::encode_path(&autosave))),
            "restored, it goes back among the autosaves: {info}"
        );

        // Restore both, as a file manager does: each to the place its record names.
        std::fs::rename(trash.join("files/Mine.fac"), &path).expect("restore the preset");
        std::fs::rename(&edits, &autosave).expect("restore its edits");
        store.rescan();
        assert!(
            store.find("Mine").expect("listed again").modified,
            "with its *"
        );
        let (loaded, from_autosave) = store.load("Mine").expect("load");
        assert!(from_autosave);
        assert_eq!(loaded.main_midi[0], 43, "the edits are back");
        assert_eq!(store.load_saved("Mine").expect("saved").main_midi[0], 42);
    }

    #[test]
    fn a_delete_the_trash_cannot_take_keeps_the_preset_its_edits_and_the_overwrites_backup() {
        // The trash under a path that is a file: nothing can be made there, as when it is on
        // another filesystem. The fallback used to rename the preset over the `.bak` an earlier
        // overwrite had left, and to delete the autosave.
        let tmp = tempdir("delete-no-trash");
        let blocked = tmp.join("data");
        std::fs::write(&blocked, b"not a directory").expect("block the trash");
        let mut store =
            PresetStore::with_dirs(Vec::new(), tmp.join("user")).with_trash(blocked.join("Trash"));
        let mut mine = Preset {
            name: "Mine".into(),
            ..Preset::default()
        };
        mine.main_midi[0] = 10;
        store.save_as(&mine, "Mine").expect("save");
        mine.main_midi[0] = 99;
        let path = store.save_as(&mine, "Mine").expect("overwrite");
        let overwritten = tmp.join("user/Mine.fac.bak");
        assert_eq!(
            crate::load(&overwritten)
                .expect("the overwrite's backup")
                .main_midi[0],
            10
        );
        mine.main_midi[0] = 43;
        store.autosave(&mine).expect("autosave");

        let went = store
            .delete("Mine")
            .expect("delete")
            .expect("it was listed");
        let aside = tmp.join("user/Mine.fac.1.bak");
        assert_eq!(went, crate::trash::Discarded::Backup(aside.clone()));
        assert!(!path.exists());
        assert_eq!(
            crate::load(&aside).expect("the deleted preset").main_midi[0],
            99
        );
        assert_eq!(
            crate::load(&overwritten)
                .expect("the overwrite's backup survives")
                .main_midi[0],
            10
        );
        let edits = tmp.join("user/AutoSave/Mine.fac.1.bak");
        assert_eq!(
            crate::load(&edits)
                .expect("the unsaved edits survive")
                .main_midi[0],
            43
        );
        assert!(!store.autosave_path("Mine").exists());
        assert!(store.entries().is_empty(), "{:?}", store.entries());

        // A second overwrite of a new Mine takes only its own `.bak`, never the deleted one.
        store.save_as(&mine, "Mine").expect("a new Mine");
        store.save_as(&mine, "Mine").expect("overwrite it");
        assert_eq!(
            crate::load(&aside)
                .expect("still the deleted preset")
                .main_midi[0],
            99
        );
    }

    #[test]
    fn a_rename_onto_an_unlisted_file_sets_it_aside_beside_the_bak_already_there() {
        let tmp = tempdir("rename-aside");
        let user = tmp.join("user");
        let mut store = PresetStore::with_dirs(Vec::new(), user.clone());
        let mut mine = Preset {
            name: "Mine".into(),
            ..Preset::default()
        };
        mine.main_midi[0] = 42;
        store.save_as(&mine, "Mine").expect("save");
        // Under the new name: a file the list cannot read, and an older `.bak` of it.
        std::fs::write(user.join("New.fac"), b"not a preset").expect("unlisted");
        std::fs::write(user.join("New.fac.bak"), b"older").expect("backup");
        store.rescan();
        assert!(store.find("New").is_none());

        store.rename("Mine", "New").expect("rename");
        assert_eq!(
            crate::load(&user.join("New.fac"))
                .expect("renamed")
                .main_midi[0],
            42
        );
        assert_eq!(std::fs::read(user.join("New.fac.bak")).unwrap(), b"older");
        assert_eq!(
            std::fs::read(user.join("New.fac.1.bak")).unwrap(),
            b"not a preset"
        );
    }

    #[test]
    fn a_rename_that_only_changes_case_keeps_the_one_file() {
        // 0.4.0 audit #19. The original saves a copy and deletes the old file; where the
        // filesystem folds case, `rock.fac` and `Rock.fac` are one file and that deleted it.
        let tmp = tempdir("rename-case");
        let mut store = PresetStore::with_dirs(Vec::new(), tmp.join("user"));
        let mut rock = Preset {
            name: "rock".into(),
            ..Preset::default()
        };
        rock.main_midi[0] = 77;
        let old_path = store.save_as(&rock, "rock").expect("save");

        store.rename("rock", "Rock").expect("rename");
        let names: Vec<_> = store.entries().iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, ["Rock"]);
        let entry = store.find("Rock").expect("listed under the new name");
        assert!(entry.path.ends_with("Rock.fac"), "{}", entry.path.display());
        assert!(!old_path.exists() || same_file(&old_path, &entry.path));
        let reloaded = crate::load(&entry.path).expect("load");
        assert_eq!(reloaded.name, "Rock", "the name inside moved too");
        assert_eq!(reloaded.main_midi[0], 77);
        let files: Vec<_> = std::fs::read_dir(tmp.join("user"))
            .expect("read")
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "fac"))
            .collect();
        assert_eq!(files.len(), 1, "one file, not a copy beside the original");
    }

    #[test]
    fn a_rename_moves_the_autosave_with_the_preset() {
        let tmp = tempdir("rename-autosave");
        let mut store = PresetStore::with_dirs(Vec::new(), tmp.join("user"));
        let mut preset = Preset {
            name: "Old".into(),
            ..Preset::default()
        };
        store.save_as(&preset, "Old").expect("save");
        preset.main_midi[0] = 99;
        store.autosave(&preset).expect("autosave");
        store.rescan();

        store.rename("Old", "New").expect("rename");
        assert!(!store.autosave_path("Old").exists());
        let (loaded, from_autosave) = store.load("New").expect("load");
        assert!(from_autosave, "the unsaved edits came along");
        assert_eq!(loaded.main_midi[0], 99);
        assert_eq!(loaded.name, "New");
        assert!(store.find("New").expect("listed").modified);
    }

    #[test]
    fn a_rename_onto_a_name_with_a_leftover_autosave_leaves_the_preset_unmodified() {
        // FA: 0.3.0 stashed microphone curves as `.fac` autosaves under voice preset names; a
        // clean preset renamed to one of them inherited it as its own unsaved changes.
        let tmp = tempdir("rename-leftover-autosave");
        let mut store = PresetStore::with_dirs(Vec::new(), tmp.join("user"));
        let preset = Preset {
            name: "My EQ".into(),
            main_midi: [10, 0, 0, 0, 0, 0],
            ..Preset::default()
        };
        store.save_as(&preset, "My EQ").expect("save");
        let leftover = Preset {
            name: "Podcast".into(),
            main_midi: [77, 0, 0, 0, 0, 0],
            ..Preset::default()
        };
        store.autosave(&leftover).expect("the leftover");
        store.rescan();
        assert!(!store.find("My EQ").expect("listed").modified);

        store.rename("My EQ", "Podcast").expect("rename");

        let entry = store.find("Podcast").expect("listed");
        assert!(!entry.modified, "no unsaved changes came from the leftover");
        assert!(!store.autosave_path("Podcast").exists());
        let (loaded, from_autosave) = store.load("Podcast").expect("load");
        assert!(!from_autosave);
        assert_eq!(loaded.main_midi, preset.main_midi, "the preset's own curve");
        let trashed = tmp.join("user/.Trash/files");
        assert!(
            std::fs::read_dir(&trashed).is_ok_and(|mut files| files.next().is_some()),
            "the leftover went to the trash, not nowhere"
        );
    }

    #[test]
    fn a_rename_onto_another_presets_file_is_refused_and_changes_nothing() {
        let tmp = tempdir("rename-shared");
        let mut store = PresetStore::with_dirs(Vec::new(), tmp.join("user"));
        let preset = Preset::default();
        store.save_as(&preset, "Music").expect("save Music");
        store.save_as(&preset, "Mine").expect("save Mine");
        let err = store.rename("Mine", "Mu:sic").unwrap_err();
        assert!(
            matches!(&err, PresetError::SharedFile { existing, .. } if existing == "Music"),
            "{err}"
        );
        let mut names: Vec<_> = store.entries().iter().map(|e| e.name.clone()).collect();
        names.sort();
        assert_eq!(names, ["Mine", "Music"]);
        // Nor onto another preset's own name: that preset's file would be put aside for it.
        assert!(store.rename("Mine", "Music").is_err());
        let music = store.find("Music").expect("still listed").path.clone();
        assert!(music.is_file());
        assert!(store.find("Mine").is_some());
        // A factory preset has no file of the user's to rename.
        let mut with_factory = store_in(&tmp.join("factory-case"));
        assert!(with_factory.rename("Jazz", "Jazz 2").is_err());
    }

    #[test]
    fn an_import_never_writes_over_a_user_preset_or_a_file_already_in_the_user_directory() {
        // FA: two files that come out as one new name went through `save_as`'s overwrite, and the
        // second replaced the first, which was left only in a `.bak` the list does not show.
        let tmp = tempdir("import-never-over");
        let mut store = PresetStore::with_dirs(Vec::new(), tmp.join("user"));
        let from = tmp.join("from");
        std::fs::create_dir_all(from.join("again")).expect("mkdir");
        let first = Preset {
            name: "first".into(),
            main_midi: [10, 0, 0, 0, 0, 0],
            ..Preset::default()
        };
        let second = Preset {
            name: "second".into(),
            main_midi: [20, 0, 0, 0, 0, 0],
            ..Preset::default()
        };
        crate::save(&first, &from.join("Night.fac")).expect("write the first");
        crate::save(&second, &from.join("again/Night.fac")).expect("write the second");

        assert_eq!(
            store.import(&from.join("Night.fac")).expect("import"),
            "Night"
        );
        let err = store.import(&from.join("again/Night.fac")).unwrap_err();
        assert!(
            matches!(&err, PresetError::Io(io) if io.kind() == std::io::ErrorKind::AlreadyExists),
            "{err}"
        );
        let (kept, _) = store.load("Night").expect("load");
        assert_eq!(kept.main_midi, first.main_midi, "the first import stays");
        assert!(
            !tmp.join("user/Night.fac.bak").exists(),
            "nothing was set aside"
        );

        // A file in the user directory the list does not hold is not written over either.
        std::fs::write(tmp.join("user/Loose.fac"), "not a preset").expect("write");
        store.rescan();
        crate::save(&first, &from.join("Loose.fac")).expect("write the source");
        assert!(store.import(&from.join("Loose.fac")).is_err());
        assert_eq!(
            std::fs::read_to_string(tmp.join("user/Loose.fac")).expect("read"),
            "not a preset"
        );
    }

    #[test]
    fn an_import_may_still_stand_in_for_a_factory_preset_of_the_same_name() {
        let tmp = tempdir("import-over-factory");
        let mut store = store_in(&tmp);
        assert_eq!(
            store.find("Jazz").expect("listed").source,
            PresetSource::Factory
        );
        let source_dir = tmp.join("from");
        std::fs::create_dir_all(&source_dir).expect("mkdir");
        crate::save(&Preset::default(), &source_dir.join("Jazz.fac")).expect("write the source");

        assert_eq!(
            store.import(&source_dir.join("Jazz.fac")).expect("import"),
            "Jazz"
        );
        assert_eq!(
            store.find("Jazz").expect("listed").source,
            PresetSource::User
        );
    }

    // Unix only: Windows cannot name the source `Mu:sic?.fac` to begin with; the sanitiser's own
    // tests cover the name on every system.
    #[cfg(unix)]
    #[test]
    fn an_imported_file_is_saved_under_a_name_windows_can_file_and_carries_it_inside() {
        // 0.4.0 audit #15: `a:b.fac` became `ab.fac` with `a:b` inside it.
        let tmp = tempdir("import-sanitised");
        let mut store = PresetStore::with_dirs(Vec::new(), tmp.join("user"));
        let source_dir = tmp.join("from");
        std::fs::create_dir_all(&source_dir).expect("mkdir");
        let source = source_dir.join("Mu:sic?.fac");
        crate::save(
            &Preset {
                name: "whatever".into(),
                ..Preset::default()
            },
            &source,
        )
        .expect("write the source");

        let name = store.import(&source).expect("import");
        assert_eq!(name, "Music");
        let entry = store.find("Music").expect("listed");
        assert!(entry.path.ends_with("Music.fac"));
        assert_eq!(crate::load(&entry.path).expect("load").name, "Music");
    }

    #[test]
    fn a_twenty_band_curve_is_exported_on_the_windows_ladder_band_for_band() {
        // 0.4.0 audit R4: CrossFX's twenty bands sit every half octave; a `.fac` for the original
        // FxSound for Windows carries the ladder it tunes twenty bands to.
        let tmp = tempdir("export-twenty");
        let mut store = PresetStore::with_dirs(Vec::new(), tmp.join("user"));
        let preset = twenty_band_preset("Twenty", &fxsound_core::eq::TWENTY_BAND_CENTRES_HZ);
        let saved = store.save_as(&preset, "Twenty").expect("save");
        assert_eq!(
            crate::load(&saved).expect("load").eq_bands,
            preset.eq_bands,
            "the user's own file keeps CrossFX's ladder"
        );

        let out = tmp.join("out");
        let path = store.export("Twenty", &out).expect("export");
        let exported = crate::load(&path).expect("parse the export");
        let centres: Vec<f32> = exported.eq_bands.iter().map(|b| b.center_hz).collect();
        assert_eq!(centres, fxsound_core::eq::WINDOWS_TWENTY_BAND_CENTRES_HZ);
        let gains = |p: &Preset| p.eq_bands.iter().map(|b| b.boost_db).collect::<Vec<_>>();
        assert_eq!(gains(&exported), gains(&preset), "every gain on its band");

        // A ten-band preset is exported as it is.
        let ten = Preset {
            name: "Ten".into(),
            ..Preset::default()
        };
        store.save_as(&ten, "Ten").expect("save ten");
        let path = store.export("Ten", &out).expect("export ten");
        assert_eq!(crate::load(&path).expect("load").eq_bands, ten.eq_bands);
    }

    #[test]
    fn end_bands_tuned_past_the_windows_ranges_are_exported_inside_them() {
        // 0.4.0 audit R6: here band 1 of ten reaches down to 46 Hz and band 10 up to 20 kHz; the
        // Windows build's wheels stop at 62.5 Hz and 16 kHz.
        let tmp = tempdir("export-wide-ends");
        let mut store = PresetStore::with_dirs(Vec::new(), tmp.join("user"));
        let mut wide = Preset {
            name: "Wide".into(),
            ..Preset::default()
        };
        wide.eq_bands[0] = fxsound_core::EqBand::new(46.0, 6.0);
        wide.eq_bands[9] = fxsound_core::EqBand::new(20000.0, -3.0);
        let saved = store.save_as(&wide, "Wide").expect("save");
        assert_eq!(
            crate::load(&saved).expect("load").eq_bands,
            wide.eq_bands,
            "the user's own file keeps where the bands were tuned"
        );

        let path = store.export("Wide", &tmp.join("out")).expect("export");
        let exported = crate::load(&path).expect("parse the export");
        assert_eq!(exported.eq_bands[0], fxsound_core::EqBand::new(62.5, 6.0));
        assert_eq!(
            exported.eq_bands[9],
            fxsound_core::EqBand::new(16000.0, -3.0)
        );
        assert_eq!(
            exported.eq_bands[1..9],
            wide.eq_bands[1..9],
            "the rest as it is"
        );
    }

    #[test]
    fn end_bands_exported_as_they_are_keep_where_they_were_tuned() {
        // «Like the original FxSound LLC» = Interface and sound, the export window's choice
        // (roadmap §14 #56): the end bands stay where they are, and only the equalizer's own window
        // limits a centre.
        let tmp = tempdir("export-as-they-are");
        let mut store = PresetStore::with_dirs(Vec::new(), tmp.join("user"));
        let mut wide = Preset {
            name: "Wide".into(),
            ..Preset::default()
        };
        wide.eq_bands[0] = fxsound_core::EqBand::new(46.0, 6.0);
        wide.eq_bands[9] = fxsound_core::EqBand::new(20000.0, -3.0);
        store.save_as(&wide, "Wide").expect("save");
        let path = store
            .export_with("Wide", &tmp.join("out"), EndBands::AsTheyAre)
            .expect("export");
        assert_eq!(crate::load(&path).expect("parse").eq_bands, wide.eq_bands);

        // A centre the equalizer would not design at goes where it is played.
        wide.eq_bands[0].center_hz = 5.0;
        wide.eq_bands[9].center_hz = 30_000.0;
        store.save_as(&wide, "Wide").expect("save");
        let path = store
            .export_with("Wide", &tmp.join("out"), EndBands::AsTheyAre)
            .expect("export");
        let exported = crate::load(&path).expect("parse");
        assert_eq!(
            exported.eq_bands[0].center_hz,
            fxsound_core::eq::BAND_FLOOR_HZ
        );
        assert_eq!(
            exported.eq_bands[9].center_hz,
            fxsound_core::eq::BAND_CEILING_HZ
        );
        assert_eq!(exported.eq_bands[1..9], wide.eq_bands[1..9]);

        // Shifted is what `export` does, 0.4.0's.
        let shifted = store.export("Wide", &tmp.join("shifted")).expect("export");
        assert_eq!(
            crate::load(&shifted).expect("parse").eq_bands[0].center_hz,
            62.5
        );
        assert_eq!(EndBands::default(), EndBands::Shifted);
    }

    #[test]
    fn a_twenty_band_curve_exported_as_it_is_still_goes_onto_the_windows_ladder() {
        // The end bands are the choice; the twenty-band ladder is what the original tunes
        // twenty bands to either way (audit report R4).
        let tmp = tempdir("export-twenty-as-they-are");
        let mut store = PresetStore::with_dirs(Vec::new(), tmp.join("user"));
        let twenty = Preset {
            name: "Twenty".into(),
            eq_bands: fxsound_core::eq::TWENTY_BAND_CENTRES_HZ
                .iter()
                .map(|&hz| fxsound_core::EqBand::new(hz, 2.0))
                .collect(),
            ..Preset::default()
        };
        store.save_as(&twenty, "Twenty").expect("save");
        let path = store
            .export_with("Twenty", &tmp.join("out"), EndBands::AsTheyAre)
            .expect("export");
        let centres: Vec<f32> = crate::load(&path)
            .expect("parse")
            .eq_bands
            .iter()
            .map(|b| b.center_hz)
            .collect();
        assert_eq!(centres, fxsound_core::eq::WINDOWS_TWENTY_BAND_CENTRES_HZ);
    }

    #[test]
    fn importing_keeps_a_centre_outside_the_windows_ranges() {
        // The wider end bands are read back as they are: an import is not an export.
        let tmp = tempdir("import-wide-ends");
        let mut wide = Preset {
            name: "Wide".into(),
            ..Preset::default()
        };
        wide.eq_bands[0].center_hz = 46.0;
        wide.eq_bands[9].center_hz = 20000.0;
        let source = tmp.join("Wide.fac");
        crate::save(&wide, &source).expect("write the source");

        let mut store = PresetStore::with_dirs(Vec::new(), tmp.join("user"));
        store.rescan();
        let name = store.import(&source).expect("import");
        let entry = store.find(&name).expect("listed");
        assert_eq!(
            crate::load(&entry.path).expect("load").eq_bands,
            wide.eq_bands
        );
    }

    #[test]
    fn a_shipped_preset_is_exported_as_it_came_unless_its_first_band_is_below_the_ladder() {
        // Every preset the original ships starts at 62.5 Hz and ends at or below 16 kHz, where
        // its wheels stop, and goes back out exactly as it came. Two CrossFX revoiced reach
        // lower, and a `.fac` for Windows carries their first band at 62.5 Hz (0.4.0 audit R6).
        let tmp = tempdir("export-shipped");
        let store = store_in(&tmp);
        assert!(store.entries().len() > 30, "the factory presets are there");
        let out = tmp.join("out");
        let mut moved = Vec::new();
        for entry in store.entries() {
            let original = crate::load(&entry.path).expect("load a shipped preset");
            let path = store.export(&entry.name, &out).expect("export");
            let exported = crate::load(&path).expect("parse the export");
            if exported.eq_bands == original.eq_bands {
                continue;
            }
            moved.push(entry.name.clone());
            assert!(original.eq_bands[0].center_hz < 62.5, "{}", entry.name);
            assert_eq!(exported.eq_bands[0].center_hz, 62.5, "{}", entry.name);
            assert_eq!(
                exported.eq_bands[0].boost_db, original.eq_bands[0].boost_db,
                "{}",
                entry.name
            );
            assert_eq!(
                exported.eq_bands[1..],
                original.eq_bands[1..],
                "{}",
                entry.name
            );
        }
        moved.sort();
        assert_eq!(moved, ["Competitive FPS", "Trap"]);
    }

    #[test]
    fn a_store_on_explicit_directories_keeps_what_it_deletes_to_itself() {
        // A test's deletion must never reach the desktop's trash, and the store's own trash must
        // never show up in its list.
        let tmp = tempdir("private-trash");
        let mut store = PresetStore::with_dirs(Vec::new(), tmp.join("user"));
        store.save_as(&Preset::default(), "Gone").expect("save");
        store.delete("Gone").expect("delete");
        assert!(tmp.join("user/.Trash/files/Gone.fac").is_file());
        store.rescan();
        assert!(store.entries().is_empty(), "{:?}", store.entries());
        assert!(PresetStore::with_default_dirs().trash_dir.is_none());
    }
}
