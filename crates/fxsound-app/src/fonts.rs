//! Settings ▸ General ▸ "Interface font", the application's side: what is chosen, the Gilroy the
//! user has, the files they add, and the designer's store.
//!
//! FxSound sets its window in Manrope, which it ships, or in Gilroy, the original FxSound's face,
//! which it neither ships nor downloads (`fxsound_ui::typeface`). A Gilroy is looked for at start
//! and whenever the option moves to it: first among the files the user added, which "Add Gilroy
//! files…" copies into FxSound's own data folder (`Settings::user_font_dir`), then on the system
//! (`fxsound_core::fonts::system_font_dirs`). What is found goes to the window's typeface, which
//! sets the window in it from the next frame on — or keeps Manrope while it is not all there.
//!
//! The file picker runs on a thread of its own, as the import window's folder picker does, and its
//! answer is taken on the next frame the Settings window draws ([`refresh`]).
//!
//! Three calls are all the controller makes: [`install`] when it starts, [`act`] for each action
//! of the pane, and [`refresh`] for each frame the pane is open.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use fxsound_core::i18n::{self, tr};
use fxsound_core::{InterfaceFont, Settings};
use fxsound_ui::dialogs::settings::{
    FontAction, FontMessage, GILROY_STORE, GilroyFound, SettingsState,
};
use fxsound_ui::typeface::{self, FaceReading, GilroyFace, GilroyFaces};

/// The file picker's title.
const PICKER_TITLE: &str = "Choose the Gilroy files you licensed";

/// What the pane is told, and the picker on its way.
struct Pane {
    found: GilroyFound,
    message: Option<FontMessage>,
    picker: Option<crossbeam_channel::Receiver<Option<Vec<PathBuf>>>>,
}

static PANE: Mutex<Pane> = Mutex::new(Pane {
    found: GilroyFound::None,
    message: None,
    picker: None,
});

fn pane() -> std::sync::MutexGuard<'static, Pane> {
    PANE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Held while Gilroy is looked for, so that one look at a time reads the folders and the last to
/// finish is the one that read them last: the look at start runs on a thread of its own, the
/// pane's on the window's. Taken before [`PANE`], never while holding it.
static LOOKING: Mutex<()> = Mutex::new(());

/// Where [`find`] looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Folders {
    /// FxSound's own folder only: what "Add Gilroy files…" put there.
    Own,
    /// FxSound's own folder, then the system's.
    All,
}

/// At start: the face `settings.toml` chose, and the Gilroy there is.
///
/// The files the user added are read at once, so that a window set in them opens in them. The
/// system's font folders are listed on a thread of their own, and only when FxSound's own folder
/// is not a whole Gilroy: `/usr/share/fonts` can be thousands of files, read from a cold disk at
/// login. A Gilroy found there shows from the next frame the window draws.
pub fn install(settings: &Settings) {
    typeface::set_choice(settings.interface_font);
    if find(Folders::Own) == GilroyFound::Whole {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("fxsound-fonts".into())
        .spawn(|| {
            find(Folders::All);
        });
    if let Err(error) = spawned {
        log::warn!("could not look for Gilroy among the system's fonts: {error}");
    }
}

/// Act on what the pane asked for. Returns whether `settings` changed and are to be saved.
pub fn act(action: &FontAction, settings: &mut Settings, state: &mut SettingsState) -> bool {
    match action {
        FontAction::Choose(font) => {
            settings.interface_font = *font;
            state.settings.interface_font = *font;
            typeface::set_choice(*font);
            pane().message = None;
            // A Gilroy installed since FxSound started is found the moment it is chosen.
            if *font == InterfaceFont::Gilroy {
                find(Folders::All);
            }
            true
        }
        FontAction::Buy => {
            if let Err(error) = open_store() {
                log::warn!("could not open {GILROY_STORE}: {error}");
                pane().message = Some(FontMessage::NoBrowser);
            }
            false
        }
        FontAction::AddFiles => {
            start_picker();
            false
        }
    }
}

/// Bring the pane up to date: take the picker's answer if it came, and say what was found.
pub fn refresh(state: &mut SettingsState) {
    let answer = {
        let mut pane = pane();
        match pane
            .picker
            .as_ref()
            .map(crossbeam_channel::Receiver::try_recv)
        {
            Some(Ok(answer)) => {
                pane.picker = None;
                answer
            }
            // Gone without a word: as good as cancelled.
            Some(Err(crossbeam_channel::TryRecvError::Disconnected)) => {
                pane.picker = None;
                None
            }
            Some(Err(crossbeam_channel::TryRecvError::Empty)) | None => None,
        }
    };
    if let Some(files) = answer {
        add(&files);
    }
    let pane = pane();
    state.font.gilroy = pane.found;
    state.font.message.clone_from(&pane.message);
    state.font.lacks_letters =
        typeface::gilroy().is_some_and(|gilroy| !gilroy.sets(&i18n::current()));
}

/// Look for Gilroy in `folders`, hand what was found to the window's typeface, and tell the pane
/// how much of it there is.
///
/// FxSound's own folder is read whole, whatever its files are called; the system's are read for
/// files whose names say Gilroy. Either way, a file counts only for what its name table says.
fn find(folders: Folders) -> GilroyFound {
    let _looking = LOOKING.lock().unwrap_or_else(PoisonError::into_inner);
    let mut files = fxsound_core::fonts::font_files(&Settings::user_font_dir(), "");
    if folders == Folders::All {
        for dir in fxsound_core::fonts::system_font_dirs() {
            files.extend(fxsound_core::fonts::font_files(&dir, "gilroy"));
        }
    }
    let mut faces: Vec<(GilroyFace, Vec<u8>)> = Vec::new();
    for file in files {
        if let FaceReading::Gilroy(face) = typeface::read_face(&file.bytes) {
            log::debug!("{}: Gilroy {face:?}", file.path.display());
            faces.push((face, file.bytes));
        }
    }
    let any = !faces.is_empty();
    let found = match GilroyFaces::assemble(faces) {
        Ok(gilroy) => {
            typeface::set_gilroy(Some(gilroy));
            GilroyFound::Whole
        }
        Err(missing) => {
            typeface::set_gilroy(None);
            if any {
                log::info!("Gilroy is missing {missing:?}; the window stays in Manrope");
                GilroyFound::Incomplete
            } else {
                GilroyFound::None
            }
        }
    };
    pane().found = found;
    found
}

/// The files the user picked: each that is a Gilroy face the window uses is copied into
/// FxSound's own folder, the others are named back, and Gilroy is looked for again.
fn add(files: &[PathBuf]) {
    let folder = Settings::user_font_dir();
    let mut left_out = Vec::new();
    let mut not_copied = false;
    for path in files {
        let name = path
            .file_name()
            .map_or_else(|| OsString::from("?"), ToOwned::to_owned);
        let shown = name.to_string_lossy().into_owned();
        let bytes = match fxsound_core::fonts::read_font_file(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                log::warn!("{}: {error}", path.display());
                left_out.push(shown);
                continue;
            }
        };
        match typeface::read_face(&bytes) {
            FaceReading::Gilroy(face) => match fxsound_core::fonts::keep(&folder, &name, &bytes) {
                Ok(kept) => log::info!("Gilroy {face:?} added as {}", kept.display()),
                Err(error) => {
                    log::warn!(
                        "could not copy {} into {}: {error}",
                        path.display(),
                        folder.display()
                    );
                    not_copied = true;
                }
            },
            other => {
                log::info!("{}: {other:?}, not added", path.display());
                left_out.push(shown);
            }
        }
    }
    find(Folders::All);
    pane().message = if not_copied {
        Some(FontMessage::NotCopied)
    } else if !left_out.is_empty() {
        Some(FontMessage::LeftOut(left_out))
    } else {
        None
    };
}

/// Open the file picker for the Gilroy files, on a thread of its own: the blocking dialog would
/// freeze the window otherwise, as `start_folder_picker` explains for the import window's.
fn start_picker() {
    let mut pane = pane();
    if pane.picker.is_some() {
        return;
    }
    let (tx, rx) = crossbeam_channel::bounded(1);
    let title = tr(PICKER_TITLE);
    let start = dirs::download_dir().or_else(dirs::home_dir);
    let spawned = std::thread::Builder::new()
        .name("fxsound-font-picker".into())
        .spawn(move || {
            let mut dialog = rfd::FileDialog::new()
                .set_title(title)
                .add_filter("TrueType, OpenType", &["ttf", "otf", "TTF", "OTF"]);
            if let Some(start) = start {
                dialog = dialog.set_directory(start);
            }
            // The pane may be gone by now; nothing to do then.
            let _ = tx.send(dialog.pick_files());
        });
    match spawned {
        Ok(_) => {
            pane.picker = Some(rx);
            pane.message = None;
        }
        Err(error) => {
            log::warn!("could not start the file picker: {error}");
            pane.message = Some(FontMessage::NoPicker);
        }
    }
}

/// Open the designer's store page in the user's browser: `xdg-open`, as every link FxSound for
/// Linux opens (`app::open_url`).
#[cfg(target_os = "linux")]
fn open_store() -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(GILROY_STORE)
        .spawn()
        .map(drop)
        .map_err(|error| error.to_string())
}

/// Open the designer's store page in the user's browser.
///
/// The one call this module leaves to the application's Windows side (Windows port, `platform`):
/// the shell's opener, `pub fn open_url(url: &str) -> Result<(), String>` in `crate::platform`.
#[cfg(windows)]
fn open_store() -> Result<(), String> {
    crate::platform::open_url(GILROY_STORE)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn open_store() -> Result<(), String> {
    Err("no way to open a link on this platform".to_owned())
}
