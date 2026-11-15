//! "Run at startup" on Windows: the value `CrossFX` under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, where the original FxSound keeps a value
//! of its own (`FxController.cpp:2789-2812`), which is what the XDG autostart entry stands in for
//! on Linux.
//!
//! The value is `"<path of crossfx.exe>" --hide`: a start at log-on goes to the notification area,
//! as the autostart entry's `Exec=crossfx --hide` does. Its name is CrossFX's own
//! ([`fxsound_core::identity::PRODUCT`]), never the name of the original FxSound's value, which
//! this module neither reads nor writes: the two can be installed side by side without one's tick
//! turning the other on, and the uninstallers delete CrossFX's value by its name.
//!
//! Task Manager's Startup tab turns an entry off without deleting it, with a value of the same
//! name under `…\Explorer\StartupApproved\Run` whose first byte is odd — as a desktop's autostart
//! editor writes `Hidden=true` into the entry on Linux. An entry turned off there reads as
//! unticked, and ticking the box turns it back on.

use std::io;
use std::path::Path;

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW,
    RegSetKeyValueW,
};
use windows::core::PCWSTR;

use super::{wide, win32_error};

/// Where log-on starts what it starts.
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Where Task Manager says what it has turned off.
pub const APPROVED_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
/// The value's name, which the uninstallers delete too.
pub const VALUE_NAME: &str = fxsound_core::identity::PRODUCT;

/// What the value says for the binary at `exe`.
#[must_use]
pub fn command_for(exe: &Path) -> String {
    format!("\"{}\" --hide", exe.display())
}

/// Whether CrossFX starts at log-on: the value is there, and Task Manager has not turned it off.
#[must_use]
pub fn enabled() -> bool {
    enabled_under(RUN_KEY, APPROVED_KEY, VALUE_NAME)
}

/// Write the value for this binary, or delete it.
///
/// # Errors
///
/// If the binary's path cannot be had, or the registry refuses the change.
pub fn set(enabled: bool) -> io::Result<()> {
    set_under(
        RUN_KEY,
        APPROVED_KEY,
        VALUE_NAME,
        enabled,
        &std::env::current_exe()?,
    )
}

fn enabled_under(run: &str, approved: &str, name: &str) -> bool {
    let (run, approved, name) = (wide(run), wide(approved), wide(name));
    let mut bytes = 0_u32;
    // SAFETY: NUL-terminated strings and an out-pointer to a local; a size query, no data.
    let present = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(run.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&raw mut bytes),
        )
    } == ERROR_SUCCESS;
    if !present {
        return false;
    }
    // `02 00 00 00 …` on, `03 00 00 00` and the time it was turned off when off.
    let mut state = [0_u8; 12];
    let mut size = u32::try_from(state.len()).unwrap_or(0);
    // SAFETY: as above, with a buffer of `size` bytes.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(approved.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_BINARY,
            None,
            Some(state.as_mut_ptr().cast()),
            Some(&raw mut size),
        )
    };
    status != ERROR_SUCCESS || size == 0 || state[0] & 1 == 0
}

/// Delete `name` under `key`; one that is not there is deleted already.
fn delete(key: &[u16], name: &[u16]) -> io::Result<()> {
    // SAFETY: NUL-terminated strings that outlive the call.
    let status: WIN32_ERROR = unsafe {
        RegDeleteKeyValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
        )
    };
    match status {
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
        other => Err(win32_error(other)),
    }
}

fn set_under(run: &str, approved: &str, name: &str, enabled: bool, exe: &Path) -> io::Result<()> {
    let (run, approved, name) = (wide(run), wide(approved), wide(name));
    if !enabled {
        delete(&run, &name)?;
        // What Task Manager kept about the entry goes with it.
        return delete(&approved, &name);
    }
    let command = wide(command_for(exe));
    let bytes = u32::try_from(command.len() * size_of::<u16>())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "the path is too long"))?;
    // SAFETY: NUL-terminated strings, and `bytes` of string data, that outlive the call.
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            PCWSTR(run.as_ptr()),
            PCWSTR(name.as_ptr()),
            REG_SZ.0,
            Some(command.as_ptr().cast()),
            bytes,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(win32_error(status));
    }
    // Ticked again after Task Manager turned it off: on, as the box says.
    delete(&approved, &name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key of the test's own under `HKCU\Software`, deleted afterwards: the tests never touch
    /// the user's real `Run` key.
    struct ScratchKey(String);

    impl ScratchKey {
        fn new(tag: &str) -> Self {
            Self(format!(
                r"Software\CrossFX tests\{tag}-{}",
                std::process::id()
            ))
        }

        fn run(&self) -> String {
            format!(r"{}\Run", self.0)
        }

        fn approved(&self) -> String {
            format!(r"{}\StartupApproved", self.0)
        }
    }

    impl Drop for ScratchKey {
        fn drop(&mut self) {
            use windows::Win32::System::Registry::{RegDeleteKeyW, RegDeleteTreeW};
            let (own, parent) = (wide(&self.0), wide(r"Software\CrossFX tests"));
            // SAFETY: NUL-terminated strings; the keys are the tests' own. The parent goes only
            // once it is empty, which `RegDeleteKeyW` checks.
            unsafe {
                let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(own.as_ptr()));
                let _ = RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(own.as_ptr()));
                let _ = RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(parent.as_ptr()));
            }
        }
    }

    /// The value CrossFX writes is `CrossFX`: never the original FxSound's value, whatever its
    /// case, nor any other name of the original's.
    #[test]
    fn the_value_is_crossfx_own_and_never_the_original_s() {
        assert_eq!(VALUE_NAME, "CrossFX");
        assert!(
            !VALUE_NAME.to_lowercase().contains("fxsound"),
            "{VALUE_NAME}"
        );
    }

    #[test]
    fn the_command_quotes_the_path_and_starts_in_the_tray() {
        assert_eq!(
            command_for(Path::new(r"C:\Program Files\CrossFX\crossfx.exe")),
            r#""C:\Program Files\CrossFX\crossfx.exe" --hide"#
        );
    }

    #[test]
    fn ticking_writes_the_value_and_unticking_takes_it_and_task_managers_note_away() {
        let key = ScratchKey::new("tick");
        let exe = Path::new(r"C:\Program Files\CrossFX\crossfx.exe");
        assert!(!enabled_under(&key.run(), &key.approved(), VALUE_NAME));
        set_under(&key.run(), &key.approved(), VALUE_NAME, true, exe).expect("tick");
        assert!(enabled_under(&key.run(), &key.approved(), VALUE_NAME));

        // Task Manager turns it off.
        let (approved, name) = (wide(key.approved()), wide(VALUE_NAME));
        let off = [3_u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        // SAFETY: NUL-terminated strings and 12 bytes of data that outlive the call.
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                PCWSTR(approved.as_ptr()),
                PCWSTR(name.as_ptr()),
                windows::Win32::System::Registry::REG_BINARY.0,
                Some(off.as_ptr().cast()),
                12,
            )
        };
        assert_eq!(status, ERROR_SUCCESS);
        assert!(
            !enabled_under(&key.run(), &key.approved(), VALUE_NAME),
            "turned off in Task Manager reads as unticked"
        );
        set_under(&key.run(), &key.approved(), VALUE_NAME, true, exe).expect("tick again");
        assert!(enabled_under(&key.run(), &key.approved(), VALUE_NAME));

        set_under(&key.run(), &key.approved(), VALUE_NAME, false, exe).expect("untick");
        assert!(!enabled_under(&key.run(), &key.approved(), VALUE_NAME));
        set_under(&key.run(), &key.approved(), VALUE_NAME, false, exe).expect("again: a no-op");
    }
}
