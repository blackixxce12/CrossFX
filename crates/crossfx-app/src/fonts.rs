//! Settings ▸ General ▸ "Interface font", the application's side: what is chosen, the Gilroy the
//! user has, the files they add, and the designer's store.
//!
//! CrossFX sets its window in Manrope, which it ships, or in Gilroy, the original FxSound's face,
//! which it neither ships nor downloads (`crossfx_ui::typeface`). A Gilroy is looked for at start
//! and whenever the option moves to it: first among the files the user added, which "Add Gilroy
//! files…" copies into CrossFX's own data folder (`Settings::user_font_dir`), then — unless those
//! are a whole Gilroy already — on the system (`crossfx_core::fonts::system_font_dirs`). What is
//! found goes to the window's typeface, which the window installs from the next frame it draws
//! (`crossfx_ui::typeface::sync`) — or keeps Manrope while it is not all there.
//!
//! The window's thread reads one thing here, CrossFX's own folder at start, which a window set in
//! the user's Gilroy needs before it opens. The system's font folders — thousands of files on
//! Linux, read from a cold disk at login — are listed on a thread of their own, at start and when
//! Gilroy is chosen; the file picker runs on one too, as the import window's folder picker does,
//! and that thread goes on to read, check and copy the files it was given. Each of these wakes
//! the window once it is done (the controller's [`Waker`]), so that what it found shows at once
//! rather than at the next keepalive.
//!
//! Three calls are all the controller makes: [`install`] when it starts, [`act`] for each action
//! of the pane, and [`refresh`] for each frame the pane is open.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use crossfx_core::fonts::FontFile;
use crossfx_core::i18n::{self, tr};
use crossfx_core::{InterfaceFont, Settings};
use crossfx_ui::dialogs::settings::{
    FontAction, FontMessage, GILROY_STORE, GilroyFound, SettingsState,
};
use crossfx_ui::typeface::{self, FaceReading, GilroyFace, GilroyFaces};

use crate::wake::Waker;

/// The file picker's title.
const PICKER_TITLE: &str = "Choose the Gilroy files you licensed";

/// What the pane is told, the picker at work, and what wakes the window.
struct Pane {
    found: GilroyFound,
    message: Option<FontMessage>,
    /// The picker's thread, while it runs: nothing is ever sent on it, and it is disconnected
    /// once the thread has ended, however it ended ([`picking`]).
    picker: Option<crossbeam_channel::Receiver<()>>,
    /// The controller's, from [`install`]; none in a run that never installed, where nothing has
    /// a window to wake.
    waker: Option<Waker>,
}

static PANE: Mutex<Pane> = Mutex::new(Pane {
    found: GilroyFound::None,
    message: None,
    picker: None,
    waker: None,
});

fn pane() -> std::sync::MutexGuard<'static, Pane> {
    PANE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Held while Gilroy is looked for, so that one look at a time reads the folders and the last to
/// finish is the one that read them last: the window's at start, and the threads' after it. Taken
/// before [`PANE`], never while holding it.
static LOOKING: Mutex<()> = Mutex::new(());

/// Where [`find`] looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Folders {
    /// CrossFX's own folder only: what "Add Gilroy files…" put there.
    Own,
    /// CrossFX's own folder, then the system's unless the own folder is a whole Gilroy.
    All,
}

/// At start: the face `settings.toml` chose, the Gilroy there is, and what wakes the window when
/// a thread of this module has found something.
///
/// The files the user added are read at once, so that a window set in them opens in them. The
/// system's font folders are listed on a thread of their own, and only when CrossFX's own folder
/// is not a whole Gilroy: `/usr/share/fonts` can be thousands of files, read from a cold disk at
/// login. A Gilroy found there shows on the frame its thread wakes the window for.
pub fn install(settings: &Settings, waker: &Waker) {
    typeface::set_choice(settings.interface_font);
    pane().waker = Some(waker.clone());
    if find(Folders::Own) != GilroyFound::Whole {
        look_on_a_thread();
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
            // A Gilroy installed since CrossFX started is found the moment it is chosen — on a
            // thread, as at start, so that neither a cold font folder nor the look at start, still
            // reading one, holds up the window the click came from.
            if *font == InterfaceFont::Gilroy {
                look_on_a_thread();
            }
            true
        }
        FontAction::Buy => {
            if let Err(error) = crate::platform::open_url(GILROY_STORE) {
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

/// Bring the pane up to date: what was found, and what the last action came to. Nothing here
/// waits for a disk; the threads that read one have left what they found in `PANE`.
pub fn refresh(state: &mut SettingsState) {
    {
        let pane = pane();
        state.font.gilroy = pane.found;
        state.font.message.clone_from(&pane.message);
    }
    state.font.lacks_letters =
        typeface::gilroy().is_some_and(|gilroy| !gilroy.sets(&i18n::current()));
}

/// Look for Gilroy in `folders`, hand what was found to the window's typeface, and tell the pane
/// how much of it there is.
///
/// CrossFX's own folder is read whole: every `.ttf` and `.otf` file there, which is every file
/// "Add Gilroy files…" keeps (`crossfx_core::fonts::keep` gives a face that came without either
/// one of them). The system's are read for files whose names say Gilroy, and only when the own
/// folder is not a whole Gilroy: the first face found for each weight is the one kept, and the
/// own folder is read first, so a system's Gilroy could only stand in for what the user did not
/// add — a SemiBold where they added a Medium is the one face it would have changed. Either way,
/// a file counts only for what its name table says.
fn find(folders: Folders) -> GilroyFound {
    let _looking = LOOKING.lock().unwrap_or_else(PoisonError::into_inner);
    let own = crossfx_core::fonts::font_files(&Settings::user_font_dir(), "");
    let mut faces = gilroy_faces(own);
    if folders == Folders::All
        && !GilroyFaces::missing(faces.iter().map(|(face, _)| *face)).is_empty()
    {
        for dir in crossfx_core::fonts::system_font_dirs() {
            let files = crossfx_core::fonts::font_files(&dir, "gilroy");
            faces.extend(gilroy_faces(files));
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

/// The Gilroy faces among `files`, each with its bytes, in the files' order.
fn gilroy_faces(files: Vec<FontFile>) -> Vec<(GilroyFace, Vec<u8>)> {
    files
        .into_iter()
        .filter_map(|file| match typeface::read_face(&file.bytes) {
            FaceReading::Gilroy(face) => {
                log::debug!("{}: Gilroy {face:?}", file.path.display());
                Some((face, file.bytes))
            }
            _ => None,
        })
        .collect()
}

/// Look for Gilroy in every folder ([`Folders::All`]) on a thread of its own, then wake the
/// window to show what was found.
///
/// One thread for each look asked for: they wait for each other at [`LOOKING`], and each reads
/// the folders after whatever asked for it. The thread is not joined; it ends with its look.
fn look_on_a_thread() {
    let spawned = std::thread::Builder::new()
        .name("crossfx-fonts".into())
        .spawn(|| {
            find(Folders::All);
            wake();
        });
    if let Err(error) = spawned {
        log::warn!("could not look for Gilroy among the system's fonts: {error}");
    }
}

/// Wake the window, so that what a thread of this module has just left in [`PANE`] and in the
/// typeface shows on the next frame rather than at the next keepalive, a second later.
fn wake() {
    let waker = pane().waker.clone();
    if let Some(waker) = waker {
        waker.wake();
    }
}

/// The files the user picked: each that is a Gilroy face the window uses is copied into
/// CrossFX's own folder, the others are named back, and Gilroy is looked for again. On the
/// picker's thread: the files are read whole and each copy is synced to the disk.
fn add(files: &[PathBuf]) {
    let folder = Settings::user_font_dir();
    let mut left_out = Vec::new();
    let mut not_copied = false;
    for path in files {
        let name = path
            .file_name()
            .map_or_else(|| OsString::from("?"), ToOwned::to_owned);
        let shown = name.to_string_lossy().into_owned();
        let bytes = match crossfx_core::fonts::read_font_file(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                log::warn!("{}: {error}", path.display());
                left_out.push(shown);
                continue;
            }
        };
        match typeface::read_face(&bytes) {
            FaceReading::Gilroy(face) => match crossfx_core::fonts::keep(&folder, &name, &bytes) {
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

/// Whether the picker's thread is still at work.
fn picking(pane: &Pane) -> bool {
    pane.picker.as_ref().is_some_and(|picker| {
        picker.try_recv() != Err(crossbeam_channel::TryRecvError::Disconnected)
    })
}

/// Open the file picker for the Gilroy files, on a thread of its own: the blocking dialog would
/// freeze the window otherwise, as `start_folder_picker` explains for the import window's. The
/// same thread adds what was picked ([`add`]) and wakes the window to show it: one picker at a
/// time, from the click to the files in place.
fn start_picker() {
    let mut pane = pane();
    if picking(&pane) {
        return;
    }
    let (running, picker) = crossbeam_channel::bounded::<()>(0);
    let title = tr(PICKER_TITLE);
    let start = dirs::download_dir().or_else(dirs::home_dir);
    let spawned = std::thread::Builder::new()
        .name("crossfx-font-picker".into())
        .spawn(move || {
            // Dropped as the thread ends, which is what `picking` looks for.
            let _running = running;
            let mut dialog = rfd::FileDialog::new()
                .set_title(title)
                .add_filter("TrueType, OpenType", &["ttf", "otf", "TTF", "OTF"]);
            if let Some(start) = start {
                dialog = dialog.set_directory(start);
            }
            if let Some(files) = dialog.pick_files() {
                add(&files);
                wake();
            }
        });
    match spawned {
        Ok(_) => {
            pane.picker = Some(picker);
            pane.message = None;
        }
        Err(error) => {
            log::warn!("could not start the file picker: {error}");
            pane.message = Some(FontMessage::NoPicker);
        }
    }
}
