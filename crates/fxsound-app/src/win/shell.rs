//! Links and folders, handed to the shell: `ShellExecuteW`, which is what `xdg-open` is on Linux.
//!
//! The shell runs whatever it is given — a program's path, a document's, a `file:` URL — so
//! nothing reaches it here that has not been checked to be what the caller says it is: an
//! `http`/`https` URL, or a directory that exists (`crate::platform`).

use std::path::Path;

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
/// COM is set up on the calling thread for the call, as the shell asks of whoever calls it — the
/// GUI thread, or a toast's — apartment-threaded where the thread has no apartment yet; a thread
/// that is in one already keeps it.
fn open(target: &std::ffi::OsStr) -> Result<(), String> {
    let target = wide(target);
    // SAFETY: COM's own initialisation; balanced below only when it succeeded here.
    let initialised =
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) }.is_ok();
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
    if initialised {
        // SAFETY: balances the successful `CoInitializeEx` above, on the same thread.
        unsafe { CoUninitialize() };
    }
    shell_status(code.0 as usize)
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
}
