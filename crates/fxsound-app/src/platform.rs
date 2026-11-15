//! What FxSound asks of the desktop that each system answers in its own way: a link opened in the
//! browser, a folder shown in the file manager, a start at log-in, the desktop's language, the
//! terminal FxSound was started from.
//!
//! One function for each, the same on both systems, so that the window, the notifications and the
//! settings pane never ask which system they run on, and never spawn an opener of their own: on
//! Linux every one they spawned is a child that has to be reaped, and a copy that forgot to was a
//! zombie under FxSound — a tray application that runs for the whole session — for every click.
//!
//! | | Linux | Windows |
//! |---|---|---|
//! | [`open_url`], [`open_folder`] | `xdg-open` | `ShellExecuteW` (`win::shell`) |
//! | [`autostart_enabled`], [`set_autostart`] | `~/.config/autostart/fxsound.desktop` | `HKCU\…\Run`, `FxSound Port` (`win::autostart`) |
//! | [`adopt_desktop_language`] | the session's `LANG`, as it is | the display language, as `LANG` (`win::language`) |
//! | [`attach_parent_console`], [`release_terminal`], [`on_the_terminal`] | the terminal, as it is | `AttachConsole`, `FreeConsole` (`win::session`) |

use std::io::Write;
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
        spawn_xdg_open(std::ffi::OsStr::new(url))
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
        spawn_xdg_open(dir.as_os_str())
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
/// that it never lingers as a zombie: nothing in FxSound waits for children it does not know of,
/// and a dropped [`std::process::Child`] is not waited for.
///
/// Off FxSound's own stdin, stdout and stderr, as a child of a GUI is: what `xdg-open` and the
/// browser it may start in the foreground have to say is no business of FxSound's terminal or
/// journal. What it came to is: an exit other than success is logged once it is known. On the
/// generic path, with no desktop it recognises, `xdg-open` runs the browser in the foreground, so
/// the helper may wait as long as the browser runs; it holds nothing but the child.
#[cfg(target_os = "linux")]
fn spawn_xdg_open(target: &std::ffi::OsStr) -> Result<(), String> {
    use std::process::{Command, Stdio};
    let mut child = Command::new("xdg-open")
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| err.to_string())?;
    let shown = target.to_string_lossy().into_owned();
    let reaper = std::thread::Builder::new()
        .name("fxsound-xdg-open".into())
        .spawn(move || match child.wait() {
            Ok(status) if status.success() => {}
            Ok(status) => log::warn!("xdg-open {shown}: {status}"),
            Err(err) => log::warn!("xdg-open {shown}: {err}"),
        });
    // The target is on its way either way: the child that would not be reaped is no reason to say
    // it could not be opened.
    if let Err(err) = reaper {
        log::warn!("xdg-open was started, but nothing waits for it to end: {err}");
    }
    Ok(())
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
/// `--help` and `--version` typed in a terminal would print nowhere without it. What was
/// redirected stays where it was sent: `fxsound --status --json > status.json`, `fxsound --watch
/// | findstr preset`. A no-op where there is no parent console — FxSound started from the Start
/// menu — and on Linux, where a program always has the terminal it was started from.
pub fn attach_parent_console() {
    #[cfg(windows)]
    crate::win::session::attach_parent_console();
}

/// Let go of the terminal [`attach_parent_console`] borrowed, once this process is the running
/// FxSound and before its engine starts. The shell did not wait for a GUI program, so the
/// terminal is the next command's: a Ctrl+C typed there — for an `fxsound --watch`, say — and the
/// terminal closing must not end the FxSound that happened to be started from it, as they do not
/// end a job sent to the background on Linux; and a process attached to a console is ended by
/// either, which in the middle of the engine's start would leave the default device on FxSound's
/// own. What the start has to say after that goes through [`on_the_terminal`]. A no-op on Linux,
/// and for a console FxSound has of its own, the debug build's, whose Ctrl+C is the tray's Exit.
pub fn release_terminal() {
    #[cfg(windows)]
    crate::win::session::release_borrowed_console();
}

/// Print what the start has to say once [`release_terminal`] has let go of the terminal — the
/// report of the options FxSound was started with, `stdout` for the standard output and `stderr`
/// for the standard error, each unless it is empty — where it was typed, or where the stream was
/// redirected to. On Windows the terminal is borrowed again for as long as the printing takes, and
/// only when there is something to print on a stream that was the terminal's — a stream
/// redirected to a file, a pipe or `NUL` gets its share where it was sent: Ctrl+C typed there
/// while the report is printed stays the next command's, but one at the moment the terminal is
/// attached again or let go of, or the terminal closing meanwhile, ends FxSound (`win::session`).
/// On Linux, whose FxSound always has the terminal it was started from, and for a console of
/// FxSound's own, the report is just printed. A stream that cannot be written to — a pipe whose
/// reader has gone — costs its own line, and neither the other's nor the FxSound that is running.
pub fn on_the_terminal(stdout: &str, stderr: &str) {
    #[cfg(windows)]
    crate::win::session::on_the_terminal(stdout, stderr);
    #[cfg(not(windows))]
    print_report(stdout, stderr);
}

/// `stdout` on the standard output and `stderr` on the standard error, a line each, and neither
/// when it is empty: how FxSound prints a report of its own (`write_report`).
pub(crate) fn print_report(stdout: &str, stderr: &str) {
    let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
    write_report(&mut out, &mut err, stdout, stderr);
}

/// `stdout` and a newline on `out`, `stderr` and a newline on `err`, and nothing for an empty
/// one: [`print_report`] on the standard output and error, and the tests' on buffers of their
/// own. A stream that cannot be written to is a log line and nothing more: the report is printed
/// by a running FxSound, its engine started, and `println!` would have ended it — a release build
/// aborts on a panic — and with it the hand-back of the default device, for a pipe whose reader
/// had gone.
fn write_report(out: &mut impl Write, err: &mut impl Write, stdout: &str, stderr: &str) {
    for (stream, text, name) in [
        (out as &mut dyn Write, stdout, "output"),
        (err as &mut dyn Write, stderr, "error"),
    ] {
        if !text.is_empty()
            && let Err(e) = writeln!(stream, "{text}")
        {
            log::warn!("the report could not be printed on the standard {name}: {e}");
        }
    }
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

    #[test]
    fn a_report_is_each_string_and_a_newline_on_its_own_stream_and_nothing_for_an_empty_one() {
        for (stdout, stderr, out_wanted, err_wanted) in [
            ("power on", "a note", "power on\n", "a note\n"),
            ("power on", "", "power on\n", ""),
            ("", "no preset Gamng", "", "no preset Gamng\n"),
            ("", "", "", ""),
            ("preset Rock\npower on", "", "preset Rock\npower on\n", ""),
        ] {
            let (mut out, mut err) = (Vec::new(), Vec::new());
            write_report(&mut out, &mut err, stdout, stderr);
            assert_eq!(out, out_wanted.as_bytes(), "{stdout:?} {stderr:?}");
            assert_eq!(err, err_wanted.as_bytes(), "{stdout:?} {stderr:?}");
        }
    }

    #[test]
    fn a_stream_that_cannot_be_written_to_costs_its_own_line_and_nothing_else() {
        /// A pipe whose reader has gone.
        struct Gone;
        impl Write for Gone {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut err = Vec::new();
        write_report(&mut Gone, &mut err, "status power=true", "a note");
        assert_eq!(err, b"a note\n");
        let mut out = Vec::new();
        write_report(&mut out, &mut Gone, "status power=true", "a note");
        assert_eq!(out, b"status power=true\n");
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
