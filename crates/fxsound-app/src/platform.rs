//! What FxSound asks of the desktop to open for the user: a link in the browser, a folder in the
//! file manager.
//!
//! One function for each, so that the window, the notifications and the settings pane never
//! spawn an opener of their own: every one they spawned is a child that has to be reaped, and a
//! copy that forgot to was a zombie under FxSound — a tray application that runs for the whole
//! session — for every click.
//!
//! On Linux both are `xdg-open` ([`spawn_xdg_open`]). The Windows port answers them with the
//! shell (`ShellExecuteW`) behind the same two signatures; until it does, they say there is no
//! way to open anything.

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
    #[cfg(not(target_os = "linux"))]
    {
        Err(format!("no way to open {url} on this platform"))
    }
}

/// Show the directory `dir` in the user's file manager: `File::revealToUser()`
/// (`FxPresetExportDialog.cpp:196`).
///
/// The portal route (`org.freedesktop.portal.OpenURI.OpenDirectory`) needs a D-Bus call this
/// crate does not make, so this is the fallback `docs/spec/06-dialogs.md` §9.3 names:
/// `xdg-open <dir>`, which dispatches to the user's own file manager rather than a hardcoded one.
///
/// # Errors
///
/// A sentence for the log: the directory could not be shown.
pub fn open_folder(dir: &Path) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        spawn_xdg_open(dir.as_os_str())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(format!("no way to show {} on this platform", dir.display()))
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
