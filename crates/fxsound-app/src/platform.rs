//! What CrossFX asks of the desktop that each system answers in its own way: a link opened in the
//! browser, a folder shown in the file manager, a start at log-in, the desktop's language, the
//! terminal CrossFX was started from.
//!
//! One function for each, the same on both systems, so that the window, the notifications and the
//! settings pane never ask which system they run on, and never spawn an opener of their own: on
//! Linux every one they spawned is a child that has to be reaped, and a copy that forgot to was a
//! zombie under CrossFX — a tray application that runs for the whole session — for every click.
//!
//! | | Linux | Windows |
//! |---|---|---|
//! | [`open_url`], [`open_folder`] | `xdg-open` | `ShellExecuteW` (`win::shell`) |
//! | [`autostart_enabled`], [`set_autostart`] | `~/.config/autostart/io.github.blackixxce12.CrossFX.desktop` ([`AUTOSTART_ENTRY`]) | `HKCU\…\Run`, `CrossFX` (`win::autostart`) |
//! | [`adopt_desktop_language`] | the session's `LANG`, as it is | the display language, as `LANG` (`win::language`) |
//! | [`attach_parent_console`], [`release_terminal`], [`on_the_terminal`] | the terminal, as it is | `AttachConsole`, `FreeConsole` (`win::session`) |

use std::io::Write;
use std::path::Path;

/// Open `url` in the user's browser.
///
/// Only `http` and `https`: every caller passes a link of CrossFX's own, and anything else handed
/// on — `file:`, a path, a scheme with a handler — would be a way to run a program. Never through
/// a shell: the original's `URL::launchInDefaultBrowser()` (`FxSystemTrayView.cpp:280-283`) hands
/// the string to one, which makes any URL that reaches it a command line.
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
/// that it never lingers as a zombie: nothing in CrossFX waits for children it does not know of,
/// and a dropped [`std::process::Child`] is not waited for.
///
/// Off CrossFX's own stdin, stdout and stderr, as a child of a GUI is: what `xdg-open` and the
/// browser it may start in the foreground have to say is no business of CrossFX's terminal or
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
        .name("crossfx-xdg-open".into())
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

/// Whether CrossFX starts with the session.
///
/// Read live, as the original reads the registry (`FxController.cpp:2789-2812`), so that an entry
/// taken away by hand shows as unticked. On Linux: the XDG autostart entry exists and is not
/// `Hidden=true`, which is how a desktop's own autostart editor disables one without deleting it.
/// On Windows: the `Run` value exists and Task Manager has not turned it off (`win::autostart`).
#[must_use]
pub fn autostart_enabled() -> bool {
    #[cfg(target_os = "linux")]
    {
        dirs::config_dir().is_some_and(|config| autostart_enabled_in(&config))
    }
    #[cfg(windows)]
    {
        crate::win::autostart::enabled()
    }
}

/// Start CrossFX with the session, in the tray, or stop doing so: the XDG autostart entry on
/// Linux, the `Run` value on Windows. The original writes an `HKCU\…\Run` value; the XDG
/// equivalent is a `.desktop` file in `~/.config/autostart` (`docs/spec/07-startup-tray.md`).
///
/// # Errors
///
/// If the entry or the value cannot be written or removed.
pub fn set_autostart(enabled: bool) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        let config =
            dirs::config_dir().ok_or_else(|| std::io::Error::other("no config directory"))?;
        set_autostart_entry_in(&config, enabled)
    }
    #[cfg(windows)]
    {
        crate::win::autostart::set(enabled)
    }
}

/// The XDG autostart entry "Launch on system startup" writes on Linux, byte for byte: CrossFX,
/// started in the tray. `packaging/crossfx-autostart.desktop` is this file as the packages ship it
/// for an example (`tests/linux_files.rs` holds the two equal), so it is defined on every system.
pub const AUTOSTART_ENTRY: &str = "[Desktop Entry]\n\
    Type=Application\n\
    Name=CrossFX\n\
    Comment=Start CrossFX minimised to the system tray on login\n\
    Exec=crossfx --hide\n\
    Icon=io.github.blackixxce12.CrossFX\n\
    Terminal=false\n\
    Categories=AudioVideo;Audio;\n\
    X-GNOME-Autostart-enabled=true\n";

/// `<config>/autostart/io.github.blackixxce12.CrossFX.desktop` — the XDG Autostart entry that
/// stands in for the original's `HKCU\…\CurrentVersion\Run` value (`FxController.cpp:2789-2812`),
/// under `config`, the user's `~/.config`. It is named after the desktop id
/// ([`fxsound_core::identity::APP_ID`]), as GNOME Tweaks and KDE's Autostart page name the copies
/// they make, so that CrossFX and the desktop share one file.
#[cfg(any(target_os = "linux", test))]
fn autostart_path_in(config: &Path) -> std::path::PathBuf {
    config
        .join("autostart")
        .join(format!("{}.desktop", fxsound_core::identity::APP_ID))
}

/// Whether the entry under `config` starts CrossFX: it is there, and not `Hidden=true`.
#[cfg(any(target_os = "linux", test))]
fn autostart_enabled_in(config: &Path) -> bool {
    std::fs::read_to_string(autostart_path_in(config))
        .is_ok_and(|text| !text.lines().any(|line| line.trim() == "Hidden=true"))
}

/// Write [`AUTOSTART_ENTRY`] under `config`, or remove it.
#[cfg(any(target_os = "linux", test))]
fn set_autostart_entry_in(config: &Path, enabled: bool) -> std::io::Result<()> {
    let path = autostart_path_in(config);
    if !enabled {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        };
    }
    std::fs::create_dir_all(config.join("autostart"))?;
    std::fs::write(&path, AUTOSTART_ENTRY)
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
/// build on Windows is a GUI program, which gets no console of its own, so `crossfx --status`,
/// `--help` and `--version` typed in a terminal would print nowhere without it. What was
/// redirected stays where it was sent: `crossfx --status --json > status.json`, `crossfx --watch
/// | findstr preset`. A no-op where there is no parent console — CrossFX started from the Start
/// menu — and on Linux, where a program always has the terminal it was started from.
pub fn attach_parent_console() {
    #[cfg(windows)]
    crate::win::session::attach_parent_console();
}

/// Let go of the terminal [`attach_parent_console`] borrowed, once this process is the running
/// CrossFX and before its engine starts. The shell did not wait for a GUI program, so the
/// terminal is the next command's: a Ctrl+C typed there — for an `crossfx --watch`, say — and the
/// terminal closing must not end the CrossFX that happened to be started from it, as they do not
/// end a job sent to the background on Linux; and a process attached to a console is ended by
/// either, which in the middle of the engine's start would leave the default device on CrossFX's
/// own. What the start has to say after that goes through [`on_the_terminal`]. A no-op on Linux,
/// and for a console CrossFX has of its own, the debug build's, whose Ctrl+C is the tray's Exit.
pub fn release_terminal() {
    #[cfg(windows)]
    crate::win::session::release_borrowed_console();
}

/// Print what the start has to say once [`release_terminal`] has let go of the terminal — the
/// report of the options CrossFX was started with, `stdout` for the standard output and `stderr`
/// for the standard error, each unless it is empty — where it was typed, or where the stream was
/// redirected to. On Windows the terminal is borrowed again for as long as the printing takes, and
/// only when there is something to print on a stream that was the terminal's — a stream
/// redirected to a file, a pipe or `NUL` gets its share where it was sent: Ctrl+C typed there
/// while the report is printed stays the next command's, but one at the moment the terminal is
/// attached again or let go of, or the terminal closing meanwhile, ends CrossFX (`win::session`).
/// On Linux, whose CrossFX always has the terminal it was started from, and for a console of
/// CrossFX's own, the report is just printed. A stream that cannot be written to — a pipe whose
/// reader has gone — costs its own line, and neither the other's nor the CrossFX that is running.
pub fn on_the_terminal(stdout: &str, stderr: &str) {
    #[cfg(windows)]
    crate::win::session::on_the_terminal(stdout, stderr);
    #[cfg(not(windows))]
    print_report(stdout, stderr);
}

/// `stdout` on the standard output and `stderr` on the standard error, a line each, and neither
/// when it is empty: how CrossFX prints a report of its own (`write_report`).
pub(crate) fn print_report(stdout: &str, stderr: &str) {
    let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
    write_report(&mut out, &mut err, stdout, stderr);
}

/// `stdout` and a newline on `out`, `stderr` and a newline on `err`, and nothing for an empty
/// one: [`print_report`] on the standard output and error, and the tests' on buffers of their
/// own. A stream that cannot be written to is a log line and nothing more: the report is printed
/// by a running CrossFX, its engine started, and `println!` would have ended it — a release build
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

    /// The entry starts CrossFX in the tray under CrossFX's names: the binary, the product's name
    /// and the application id the icon is installed under.
    #[test]
    fn the_autostart_entry_starts_crossfx_in_the_tray_under_its_own_names() {
        use fxsound_core::identity::{APP_ID, BINARY, PRODUCT};
        let lines: Vec<&str> = AUTOSTART_ENTRY.lines().collect();
        assert_eq!(lines.first(), Some(&"[Desktop Entry]"));
        for wanted in [
            format!("Name={PRODUCT}"),
            format!("Comment=Start {PRODUCT} minimised to the system tray on login"),
            format!("Exec={BINARY} --hide"),
            format!("Icon={APP_ID}"),
        ] {
            assert!(
                lines.contains(&wanted.as_str()),
                "{wanted:?} in\n{AUTOSTART_ENTRY}"
            );
        }
        assert!(AUTOSTART_ENTRY.ends_with('\n'));
        assert!(!AUTOSTART_ENTRY.to_lowercase().contains("fxsound"));
    }

    /// "Launch on system startup", in a configuration folder of the test's own: ticking writes
    /// exactly [`AUTOSTART_ENTRY`] under the desktop id's name, and nothing else, and unticking
    /// takes it away again.
    #[test]
    fn ticking_writes_the_autostart_entry_under_the_desktop_id_and_unticking_removes_it() {
        let config = tempfile::tempdir().expect("temp dir");
        let folder = config.path().join("autostart");
        let entry = folder.join("io.github.blackixxce12.CrossFX.desktop");
        assert_eq!(autostart_path_in(config.path()), entry);
        assert!(!autostart_enabled_in(config.path()));

        set_autostart_entry_in(config.path(), true).expect("tick");
        assert_eq!(
            std::fs::read_to_string(&entry).expect("the entry"),
            AUTOSTART_ENTRY
        );
        let names: Vec<String> = std::fs::read_dir(&folder)
            .expect("the autostart folder")
            .map(|item| {
                item.expect("an entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names, ["io.github.blackixxce12.CrossFX.desktop"]);
        assert!(autostart_enabled_in(config.path()));

        set_autostart_entry_in(config.path(), false).expect("untick");
        assert!(!entry.exists());
        assert!(!autostart_enabled_in(config.path()));
        set_autostart_entry_in(config.path(), false).expect("again: a no-op");
    }

    /// A desktop's autostart editor turns an entry off by writing `Hidden=true` into it rather
    /// than deleting it: the box then reads unticked, and ticking it writes the entry afresh.
    #[test]
    fn an_autostart_entry_a_desktop_turned_off_reads_as_unticked() {
        let config = tempfile::tempdir().expect("temp dir");
        set_autostart_entry_in(config.path(), true).expect("tick");
        let entry = autostart_path_in(config.path());
        std::fs::write(&entry, format!("{AUTOSTART_ENTRY}Hidden=true\n")).expect("turned off");
        assert!(!autostart_enabled_in(config.path()));

        set_autostart_entry_in(config.path(), true).expect("tick again");
        assert!(autostart_enabled_in(config.path()));
        assert_eq!(
            std::fs::read_to_string(&entry).expect("the entry"),
            AUTOSTART_ENTRY
        );
    }

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
        let file = dir.path().join("crossfx.exe");
        std::fs::write(&file, b"").expect("write");
        assert!(open_folder(&file).is_err(), "a file would be run");
        assert!(open_folder(&dir.path().join("missing")).is_err());
    }
}
