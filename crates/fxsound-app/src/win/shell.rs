//! Links and folders, handed to the shell: `ShellExecuteW`, which is what `xdg-open` is on Linux.
//!
//! The shell runs whatever it is given — a program's path, a document's, a `file:` URL — so
//! nothing reaches it here that has not been checked to be what the caller says it is: an
//! `http`/`https` URL, or a directory that exists (`crate::platform`).

use std::path::Path;

use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{PCWSTR, w};

use super::wide;

/// `ShellExecuteW`'s own status: anything above 32 is success, the rest a reason.
fn shell_status(code: usize) -> Result<(), String> {
    match code {
        33.. => Ok(()),
        // The `SE_ERR_*` values the shell documents for this call.
        0 | 8 => Err("the system is out of memory".to_owned()),
        2 => Err("the file was not found".to_owned()),
        3 => Err("the path was not found".to_owned()),
        5 => Err("access was denied".to_owned()),
        26 => Err("a sharing violation occurred".to_owned()),
        27 | 31 => Err("nothing is set up to open it".to_owned()),
        28..=30 => Err("the program that opens it did not answer".to_owned()),
        other => Err(format!("the shell refused it (code {other})")),
    }
}

/// Open `target` with the verb `open`, as a double click in Explorer would.
///
/// The shell may hand the call to an extension that wants an apartment-threaded caller, so it is
/// made from one ([`apartment_threaded`]).
fn open(target: &std::ffi::OsStr) -> Result<(), String> {
    let target = wide(target);
    apartment_threaded(move || {
        // SAFETY: NUL-terminated strings that outlive the call; no window owns the error UI.
        let code = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                PCWSTR(target.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        shell_status(code.0 as usize)
    })?
}

/// Run `f` on an apartment-threaded thread: this one, with COM set up that way for the call where
/// the thread has no apartment yet — the GUI thread is in one already, and stays as it is — or a
/// thread of its own where this one is in the multithreaded apartment, which cannot be made one.
/// A click on a toast is answered on such a thread (`crate::win::toast`).
fn apartment_threaded<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    // SAFETY: COM's own initialisation; balanced below only when it succeeded here.
    let apartment =
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    if apartment == RPC_E_CHANGED_MODE {
        return std::thread::Builder::new()
            .name("fxsound-shell".to_owned())
            .spawn(move || apartment_threaded(f))
            .map_err(|err| format!("no thread to ask the shell from: {err}"))?
            .join()
            .unwrap_or_else(|_| Err("the shell's thread panicked".to_owned()));
    }
    let done = f();
    if apartment.is_ok() {
        // SAFETY: balances the successful `CoInitializeEx` above, on the same thread.
        unsafe { CoUninitialize() };
    }
    Ok(done)
}

/// Open an `http`/`https` URL in the user's browser.
///
/// # Errors
///
/// What the shell said, when it could not open it.
pub fn open_url(url: &str) -> Result<(), String> {
    open(std::ffi::OsStr::new(url))
}

/// Show a directory in Explorer.
///
/// # Errors
///
/// What the shell said, when it could not open it.
pub fn open_folder(dir: &Path) -> Result<(), String> {
    open(dir.as_os_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Com::{
        APTTYPE, APTTYPE_MAINSTA, APTTYPE_STA, APTTYPEQUALIFIER, COINIT_MULTITHREADED,
        CoGetApartmentType,
    };

    #[test]
    fn the_shells_codes_read_as_reasons_and_above_32_as_done() {
        assert_eq!(shell_status(42), Ok(()));
        assert_eq!(shell_status(33), Ok(()));
        assert_eq!(
            shell_status(31).expect_err("no association"),
            "nothing is set up to open it"
        );
        assert!(shell_status(2).is_err());
        assert!(shell_status(32).is_err(), "32 is not above 32");
    }

    /// The apartment the calling thread is in, and the thread.
    fn where_am_i() -> (Option<APTTYPE>, std::thread::ThreadId) {
        let (mut kind, mut qualifier) = (APTTYPE::default(), APTTYPEQUALIFIER::default());
        // SAFETY: out-pointers to locals.
        let kind = unsafe { CoGetApartmentType(&raw mut kind, &raw mut qualifier) }
            .ok()
            .map(|()| kind);
        (kind, std::thread::current().id())
    }

    /// Whether `kind` is a single-threaded apartment: the main one, if this is the process's
    /// first, or another.
    const fn single(kind: Option<APTTYPE>) -> bool {
        matches!(kind, Some(APTTYPE_STA | APTTYPE_MAINSTA))
    }

    #[test]
    fn the_shell_is_asked_from_an_apartment_threaded_thread_whatever_thread_asks() {
        // A thread with no apartment is made one for the call, on the spot.
        std::thread::spawn(|| {
            let caller = std::thread::current().id();
            let (kind, thread) = apartment_threaded(where_am_i).expect("ran");
            assert!(single(kind), "{kind:?}");
            assert_eq!(thread, caller);
        })
        .join()
        .expect("the thread");
        // A thread of the multithreaded apartment, as a toast's click is answered on, hands it on.
        std::thread::spawn(|| {
            // SAFETY: COM for this thread, balanced below.
            unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
                .ok()
                .expect("the multithreaded apartment");
            let caller = std::thread::current().id();
            let (kind, thread) = apartment_threaded(where_am_i).expect("ran");
            assert!(single(kind), "{kind:?}");
            assert_ne!(thread, caller);
            // SAFETY: balances the call above.
            unsafe { CoUninitialize() };
        })
        .join()
        .expect("the thread");
    }
}
