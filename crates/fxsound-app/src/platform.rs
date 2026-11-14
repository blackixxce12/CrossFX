//! What FxSound asks of the desktop that each system answers in its own way: a link opened in the
//! browser, a folder shown in the file manager, a start at log-in.
//!
//! One function for each, the same on both systems, so that the window, the notifications and the
//! settings pane never ask which system they run on:
//!
//! | | Linux | Windows |
//! |---|---|---|
//! | [`open_url`], [`open_folder`] | `xdg-open` | `ShellExecuteW` (`win::shell`) |
//! | [`autostart_enabled`], [`set_autostart`] | `~/.config/autostart/fxsound.desktop` | `HKCU\…\Run`, `FxSound Port` (`win::autostart`) |
//! | [`adopt_desktop_language`] | the session's `LANG`, as it is | the display language, as `LANG` (`win::language`) |
//! | [`attach_parent_console`], [`release_terminal`] | the terminal, as it is | `AttachConsole`, `FreeConsole` (`win::session`) |

use std::path::Path;

/// Open `url` in the user's browser.
///
/// Only `http` and `https`: every caller passes a link of FxSound's own, and anything else handed
/// on — `file:`, a path, a scheme with a handler — would be a way to run a program. Never through
/// a shell: `URL::launchInDefaultBrowser()` (`FxSystemTrayView.cpp:280-283`) hands the string to
/// one on Windows, which makes any URL that reaches it a command line.
///
/// # Errors
///
/// A sentence for the log: the URL was refused, or the desktop could not open it.
pub fn open_url(url: &str) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("only http and https links are opened".to_owned());
    }
    #[cfg(target_os = "linux")]
    {
        spawn_xdg_open(std::ffi::OsStr::new(url), false)
    }
    #[cfg(windows)]
    {
        crate::win::shell::open_url(url)
    }
}

/// Show the directory `dir` in the user's file manager: `File::revealToUser()`
/// (`FxPresetExportDialog.cpp:196`).
///
/// On Linux the portal route (`org.freedesktop.portal.OpenURI.OpenDirectory`) needs a D-Bus call
/// this crate does not make, so this is the fallback `docs/spec/06-dialogs.md` §9.3 names:
/// `xdg-open <dir>`, which dispatches to the user's own file manager rather than a hardcoded one.
/// On Windows the shell opens a directory in Explorer — and would run a program given its path,
/// so it is given nothing that is not a directory.
///
/// # Errors
///
/// A sentence for the log: the directory could not be shown.
pub fn open_folder(dir: &Path) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        spawn_xdg_open(dir.as_os_str(), true)
    }
    #[cfg(windows)]
    {
        if !dir.is_dir() {
            return Err(format!("{} is not a directory", dir.display()));
        }
        crate::win::shell::open_folder(dir)
    }
}

/// `xdg-open <target>`, spawned directly, never through a shell, and reaped on a helper thread so
/// that it never lingers as a zombie. `quiet` for a folder, whose `xdg-open` has nothing to say
/// that the log needs.
#[cfg(target_os = "linux")]
fn spawn_xdg_open(target: &std::ffi::OsStr, quiet: bool) -> Result<(), String> {
    use std::process::{Command, Stdio};
    let mut command = Command::new("xdg-open");
    command.arg(target);
    if quiet {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
    }
    let mut child = command.spawn().map_err(|err| err.to_string())?;
    std::thread::Builder::new()
        .name("fxsound-xdg-open".into())
        .spawn(move || {
            let _ = child.wait();
        })
        .map(|_| ())
        .map_err(|err| err.to_string())
}

/// Whether FxSound starts with the session.
///
/// Read live, as the original reads the registry (`FxController.cpp:2789-2812`), so that an entry
/// taken away by hand shows as unticked. On Linux: the XDG autostart entry exists and is not
/// `Hidden=true`, which is how a desktop's own autostart editor disables one without deleting it.
/// On Windows: the `Run` value exists and Task Manager has not turned it off (`win::autostart`).
#[must_use]
pub fn autostart_enabled() -> bool {
    #[cfg(target_os = "linux")]
    {
        autostart_path()
            .ok()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .is_some_and(|text| !text.lines().any(|line| line.trim() == "Hidden=true"))
    }
    #[cfg(windows)]
    {
        crate::win::autostart::enabled()
    }
}

/// Start FxSound with the session, in the tray, or stop doing so: the XDG autostart entry on
/// Linux, the `Run` value on Windows. The Windows build writes an `HKCU\…\Run` value; the XDG
/// equivalent is a `.desktop` file in `~/.config/autostart` (`docs/spec/07-startup-tray.md`).
///
/// # Errors
///
/// If the entry or the value cannot be written or removed.
pub fn set_autostart(enabled: bool) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        set_autostart_entry(enabled)
    }
    #[cfg(windows)]
    {
        crate::win::autostart::set(enabled)
    }
}

/// `~/.config/autostart/fxsound.desktop` — the XDG Autostart entry that stands in for the
/// `HKCU\…\CurrentVersion\Run` value (`FxController.cpp:2789-2812`).
#[cfg(target_os = "linux")]
fn autostart_path() -> std::io::Result<std::path::PathBuf> {
    Ok(dirs::config_dir()
        .ok_or_else(|| std::io::Error::other("no config directory"))?
        .join("autostart")
        .join("fxsound.desktop"))
}

/// Create or remove `~/.config/autostart/fxsound.desktop`.
#[cfg(target_os = "linux")]
fn set_autostart_entry(enabled: bool) -> std::io::Result<()> {
    let path = autostart_path()?;
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("no autostart directory"))?
        .to_path_buf();

    if !enabled {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        };
    }

    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        &path,
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=FxSound\n\
         Comment=Start FxSound minimised to the system tray on login\n\
         Exec=fxsound --hide\n\
         Icon=fxsound\n\
         Terminal=false\n\
         Categories=AudioVideo;Audio;\n\
         X-GNOME-Autostart-enabled=true\n",
    )
}

/// The desktop's language, where the interface's "the system's language" reads it: on Windows the
/// user's display language, put in `LANG` for this process unless `LC_ALL`, `LC_MESSAGES` or
/// `LANG` says otherwise already (`win::language`). A no-op on Linux, whose session says it in
/// those itself. First thing in `main`, before anything reads them.
pub fn adopt_desktop_language() {
    #[cfg(windows)]
    crate::win::language::adopt_display_language();
}

/// Attach to the console of the process that started this one, when there is one: a release
/// build on Windows is a GUI program, which gets no console of its own, so `fxsound --status`,
/// `--help` and `--version` typed in a terminal would print nowhere without it. A no-op where
/// there is no parent console — FxSound started from the Start menu — and on Linux, where a
/// program always has the terminal it was started from.
pub fn attach_parent_console() {
    #[cfg(windows)]
    crate::win::session::attach_parent_console();
}

/// Let go of the terminal [`attach_parent_console`] borrowed, once this process is the running
/// FxSound and has printed what its command line had to say. The shell did not wait for a GUI
/// program, so the terminal is the next command's: a Ctrl+C typed there — for an `fxsound
/// --watch`, say — and the terminal closing must not end the FxSound that happened to be started
/// from it, as they do not end a job sent to the background on Linux. A no-op on Linux, and for a
/// console FxSound has of its own, the debug build's, whose Ctrl+C is the tray's Exit.
pub fn release_terminal() {
    #[cfg(windows)]
    crate::win::session::release_borrowed_console();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_that_is_not_http_is_refused_rather_than_handed_to_the_desktop() {
        for url in [
            "file:///etc/passwd",
            "x-scheme-handler/evil",
            r"C:\Windows\System32\calc.exe",
            "",
        ] {
            assert!(open_url(url).is_err(), "{url}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_folder_that_is_not_a_directory_is_never_handed_to_the_shell() {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("fxsound.exe");
        std::fs::write(&file, b"").expect("write");
        assert!(open_folder(&file).is_err(), "a file would be run");
        assert!(open_folder(&dir.path().join("missing")).is_err());
    }
}
