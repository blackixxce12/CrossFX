//! Windows: what the Linux session's services are to the rest of the application, on Win32.
//!
//! The Linux build leans on the desktop session for everything that is not the window: D-Bus for
//! the tray, the notifications, the bus interface and logind's suspend signal; a Unix socket under
//! `$XDG_RUNTIME_DIR` for the single instance; signals for a clean quit; `xdg-open` for links and
//! folders; an XDG autostart entry. None of that exists on Windows. Each module here stands in for
//! one of them behind the API the rest of the application already calls, so that the controller,
//! the command line and the window are the same code on both systems:
//!
//! | Linux | Windows | here |
//! |---|---|---|
//! | `$XDG_RUNTIME_DIR/crossfx/instance.sock` and its `flock` | `\\.\pipe\crossfx-<user SID>` | [`pipe`] |
//! | StatusNotifierItem (`ksni`) | the notification area (`tray-icon`) | [`tray`] |
//! | `org.freedesktop.Notifications` | a toast | [`toast`] |
//! | logind's `PrepareForSleep` | `PowerRegisterSuspendResumeNotification` | [`power`] |
//! | `SIGTERM`, `SIGINT`, `SIGHUP` | console control events, `WM_QUERYENDSESSION`/`WM_ENDSESSION` | [`session`] |
//! | `xdg-open` | `ShellExecuteW` | [`shell`] |
//! | `~/.config/autostart/io.github.blackixxce12.CrossFX.desktop` | `HKCU\…\CurrentVersion\Run`, value `CrossFX` | [`autostart`] |
//! | `--self-test`'s PipeWire and session-bus probes | the audio service and its playback endpoints | [`audio`] |
//! | `LANG` and friends, the session's language | the user's display language | [`language`] |
//!
//! There is no D-Bus interface on Windows: the control pipe, which carries every command line, is
//! the scripting surface there.
//!
//! # `unsafe`
//!
//! The crate forbids `unsafe` on Linux. Every Win32 call is `unsafe` by signature, so on Windows
//! the crate denies it instead and this module, alone, allows it (`lib.rs`). Each block wraps
//! calls whose preconditions are local — a pointer to a value on the stack that outlives the call,
//! a handle this module owns — and nothing here hands a raw pointer or a borrowed handle out.

#![allow(unsafe_code)]

pub mod audio;
pub mod autostart;
pub mod language;
pub mod pipe;
pub mod power;
pub mod session;
pub mod shell;
pub mod toast;
pub mod tray;
mod user;

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HANDLE, WAIT_EVENT, WAIT_OBJECT_0, WAIT_TIMEOUT, WIN32_ERROR};
use windows::Win32::System::Threading::{CreateEventW, ResetEvent, SetEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetSystemMetrics, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx,
    PM_REMOVE, PeekMessageW, QS_ALLINPUT, SM_CXICON, TranslateMessage, WM_QUIT,
};
use windows::core::HRESULT;

/// `s` as a NUL-terminated UTF-16 string, for a `PCWSTR`.
pub(crate) fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

/// Whether `error` is the Win32 error `code`, which the `windows` crate carries as an `HRESULT`.
pub(crate) fn is(error: &windows::core::Error, code: WIN32_ERROR) -> bool {
    error.code() == HRESULT::from_win32(code.0)
}

/// `error` as the `io::Error` the rest of the crate reads its kind from.
///
/// The `windows` crate's own conversion keeps the whole `HRESULT` as the OS error, which
/// `io::ErrorKind` does not decode — a broken pipe would come out `Uncategorized` and the control
/// pipe's callers would not see that the other end has gone. A Win32 error is handed over as its
/// own code instead.
pub(crate) fn io_error(error: &windows::core::Error) -> io::Error {
    // `HRESULT_FROM_WIN32`: severity, `FACILITY_WIN32`, then the code.
    let code = error.code().0.cast_unsigned();
    if code & 0xFFFF_0000 == 0x8007_0000 {
        io::Error::from_raw_os_error((code & 0xFFFF).cast_signed())
    } else {
        io::Error::other(error.clone())
    }
}

/// A Win32 status as an `io::Error`.
pub(crate) fn win32_error(code: WIN32_ERROR) -> io::Error {
    io::Error::from_raw_os_error(code.0.cast_signed())
}

/// A wait in milliseconds, as the Win32 waits take it: `INFINITE` for none, and anything shorter
/// than a millisecond rounded up rather than down to a poll.
pub(crate) fn millis(timeout: Option<Duration>) -> u32 {
    timeout.map_or(u32::MAX, |timeout| {
        let ms = timeout.as_nanos().div_ceil(1_000_000);
        // `u32::MAX` is `INFINITE`; a finite wait stops just short of it.
        u32::try_from(ms).unwrap_or(u32::MAX - 1).min(u32::MAX - 1)
    })
}

/// The side of a window's big icon at the system's DPI (`SM_CXICON`), in pixels: 32 at 100 %, 48
/// at 150 %. The taskbar shows a window at this size, and eframe draws the window's two icons from
/// the one picture it is given at this size and at the small icon's.
#[must_use]
pub fn big_icon_size() -> u32 {
    // SAFETY: a plain query. The process is per-monitor DPI aware (its manifest), so the size is
    // the system DPI's, as eframe reads it.
    let size = unsafe { GetSystemMetrics(SM_CXICON) };
    u32::try_from(size)
        .ok()
        .filter(|&size| size > 0)
        .unwrap_or(32)
}

/// An event object: a waitable flag, here for overlapped I/O and for the headless pump's
/// wake-up ([`crate::wake::Waker`]).
#[derive(Debug)]
pub(crate) struct Event(OwnedHandle);

impl Event {
    /// An unsignalled event that stays signalled until [`Event::reset`] (`manual`), or one that a
    /// single satisfied wait resets.
    pub(crate) fn new(manual: bool) -> io::Result<Self> {
        // SAFETY: no attributes and no name; the handle returned is ours to own.
        let handle =
            unsafe { CreateEventW(None, manual, false, None) }.map_err(|e| io_error(&e))?;
        // SAFETY: `handle` is a fresh, valid handle that nothing else owns.
        Ok(Self(unsafe { OwnedHandle::from_raw_handle(handle.0) }))
    }

    pub(crate) fn raw(&self) -> HANDLE {
        HANDLE(self.0.as_raw_handle())
    }

    pub(crate) fn set(&self) {
        // SAFETY: the handle is ours and open. A failure would mean it is not, which it is.
        let _ = unsafe { SetEvent(self.raw()) };
    }

    pub(crate) fn reset(&self) {
        // SAFETY: as for `set`.
        let _ = unsafe { ResetEvent(self.raw()) };
    }
}

/// How a wait in [`wait_pumping`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pumped {
    /// The event was set.
    Signalled,
    /// The deadline passed.
    TimedOut,
}

/// Wait for `event` until `deadline`, dispatching this thread's window messages meanwhile.
///
/// A thread that owns a window has to answer what other threads send it — `WM_QUERYENDSESSION`
/// at log-off, a broadcast `WM_SETTINGCHANGE` — or Windows counts the application as hung and the
/// sender waits on it. eframe's event loop lives on between two windows (`main.rs`, "Two states")
/// and keeps a hidden window of winit's on the GUI thread, so the headless pump, which would
/// otherwise block on its channels, waits here instead: on its waker's event and on the message
/// queue at once. winit holds whatever reaches its window outside its loop for the next one.
pub(crate) fn wait_pumping(event: &Event, deadline: Instant) -> Pumped {
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Pumped::TimedOut;
        }
        // SAFETY: one handle we own, for the length of the call.
        let woke: WAIT_EVENT = unsafe {
            MsgWaitForMultipleObjectsEx(
                Some(&[event.raw()]),
                millis(Some(left)),
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        };
        if woke == WAIT_OBJECT_0 {
            return Pumped::Signalled;
        }
        if woke == WAIT_TIMEOUT {
            return Pumped::TimedOut;
        }
        if woke.0 == WAIT_OBJECT_0.0 + 1 {
            dispatch_pending_messages();
        } else {
            // `WAIT_FAILED`: nothing to wait on is a bug, not a reason to spin.
            log::warn!(
                "waiting for the GUI thread's wake-up failed: {}",
                io::Error::last_os_error()
            );
            std::thread::sleep(left.min(Duration::from_millis(100)));
        }
    }
}

/// Dispatch every message waiting for this thread's windows. A `WM_QUIT` ends the round and is
/// dropped: nothing in CrossFX or in winit posts one to the GUI thread, and CrossFX quits by the
/// tray's Exit alone, which a stray quit message is not.
fn dispatch_pending_messages() {
    let mut msg = MSG::default();
    // SAFETY: `msg` outlives each call; messages are dispatched on the thread they were taken on.
    unsafe {
        while PeekMessageW(&raw mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_QUIT {
                break;
            }
            let _ = TranslateMessage(&raw const msg);
            DispatchMessageW(&raw const msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wait_is_rounded_up_to_a_whole_millisecond_and_none_is_infinite() {
        assert_eq!(millis(None), u32::MAX, "INFINITE");
        assert_eq!(millis(Some(Duration::from_micros(10))), 1);
        assert_eq!(millis(Some(Duration::from_millis(250))), 250);
        assert_eq!(
            millis(Some(Duration::from_secs(60 * 60 * 24 * 365))),
            u32::MAX - 1,
            "a year is long, not infinite"
        );
    }

    #[test]
    fn a_win32_error_keeps_its_own_code_and_so_its_kind() {
        let broken = windows::core::Error::from_hresult(HRESULT::from_win32(109));
        let io = io_error(&broken);
        assert_eq!(io.raw_os_error(), Some(109));
        assert_eq!(io.kind(), io::ErrorKind::BrokenPipe);
        assert!(is(&broken, windows::Win32::Foundation::ERROR_BROKEN_PIPE));
    }

    #[test]
    fn a_wait_on_a_set_event_returns_at_once_and_one_on_nothing_times_out() {
        let event = Event::new(false).expect("an event");
        event.set();
        assert_eq!(
            wait_pumping(&event, Instant::now() + Duration::from_secs(5)),
            Pumped::Signalled
        );
        let started = Instant::now();
        assert_eq!(
            wait_pumping(&event, Instant::now() + Duration::from_millis(30)),
            Pumped::TimedOut,
            "an auto-reset event is consumed by the wait it satisfied"
        );
        assert!(started.elapsed() >= Duration::from_millis(25));
        event.set();
        event.reset();
        assert_eq!(
            wait_pumping(&event, Instant::now() + Duration::from_millis(10)),
            Pumped::TimedOut
        );
    }
}
