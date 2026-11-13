//! Ending FxSound the way the tray's Exit does, whatever Windows ends it with.
//!
//! On Linux, `SIGTERM`, `SIGINT` and `SIGHUP` raise a flag that the pump reads as the tray's Exit
//! (`main.rs`): the engine hands the default devices back and stops, and only then does the
//! process end. A process that just dies leaves the system default on FxSound's virtual device,
//! with nothing behind it to play the sound on — so the Windows build needs the same for each way
//! Windows ends a program:
//!
//! - **Ctrl+C and Ctrl+Break** in the console FxSound runs in, or attached to (`AttachConsole`,
//!   `main.rs`): the flag, as `SIGINT` raises it. A second one while the way out is under way
//!   ends the process at once with the conventional 130, as a second Ctrl+C does on Linux.
//! - **The console closing**, and log-off and shutdown for a process the system tells through its
//!   console (`CTRL_CLOSE_EVENT`, `CTRL_LOGOFF_EVENT`, `CTRL_SHUTDOWN_EVENT`): the flag, and then
//!   the handler waits — the process ends as soon as it returns — until the way out has handed
//!   the devices back ([`Termination::close`]), or [`HANDBACK_WAIT`] at most.
//! - **Log-off, shutdown and a Restart Manager close** for a program with windows, which hears
//!   them as `WM_QUERYENDSESSION` and `WM_ENDSESSION` and not through its console at all: a hidden
//!   window on a thread of its own, `fxsound-session`, answers the first with "go ahead" — a
//!   sound program has no business holding the session up — and does for the second what the
//!   console's close does.
//!
//! The pump needs waking for the flag: the handlers do that through the [`Waker`], which is all
//! they do besides the flag and the wait.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT,
    CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT, SetConsoleCtrlHandler,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DispatchMessageW, GWLP_USERDATA, GetMessageW,
    GetWindowLongPtrW, MSG, PostMessageW, PostQuitMessage, RegisterClassW, SetWindowLongPtrW,
    TranslateMessage, WINDOW_EX_STYLE, WM_CLOSE, WM_DESTROY, WM_ENDSESSION, WM_NCCREATE,
    WM_NCDESTROY, WM_QUERYENDSESSION, WNDCLASSW, WS_EX_TOOLWINDOW, WS_OVERLAPPED,
};
use windows::core::{BOOL, PCWSTR, w};

use crate::wake::Waker;

/// How long a session that is ending, or a console that is closing, waits for the devices to be
/// handed back. Windows gives a program about five seconds before it offers to end it by force,
/// and the hand-back takes a few milliseconds; the rest is for a GUI thread that was busy.
pub const HANDBACK_WAIT: Duration = Duration::from_secs(4);

/// The exit status of a second Ctrl+C: 128 + `SIGINT`, the shells' convention, as on Linux.
const SECOND_CTRL_C: u32 = 130;

/// What the handlers share with the GUI thread.
#[derive(Debug)]
struct Shared {
    /// `main.rs`'s terminate flag: `true` means "quit at the next tick".
    terminate: Arc<AtomicBool>,
    waker: Waker,
    /// Set by [`Termination::close`] once the way out has handed the devices back.
    done: Mutex<bool>,
    settled: Condvar,
}

impl Shared {
    /// Ask the pump for the tray's Exit.
    fn quit(&self) {
        self.terminate.store(true, Ordering::SeqCst);
        self.waker.wake();
    }

    /// Wait until the way out is over, for at most `at_most`. Returns whether it is.
    fn wait_for_the_way_out(&self, at_most: Duration) -> bool {
        let until = Instant::now() + at_most;
        let mut done = self.done.lock().unwrap_or_else(PoisonError::into_inner);
        while !*done {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            done = self
                .settled
                .wait_timeout(done, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }

    fn finish(&self) {
        *self.done.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.settled.notify_all();
    }
}

/// The handlers' view, for the console's, which Windows calls with no context of its own.
static CONSOLE: OnceLock<Arc<Shared>> = OnceLock::new();

/// Attach to the parent process's console, for a GUI build started from a terminal: the standard
/// handles that were not redirected then reach that console, so that `fxsound --status` prints
/// where it was typed (`crate::platform::attach_parent_console`). Fails, harmlessly, for a
/// process that has a console already — the debug build, a console program — or whose parent has
/// none.
pub fn attach_parent_console() {
    // SAFETY: a plain call; its failure is the answer "no console to attach to".
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

/// The console control handler: see the module docs.
unsafe extern "system" fn on_console_event(event: u32) -> BOOL {
    let Some(shared) = CONSOLE.get() else {
        return false.into();
    };
    match event {
        CTRL_C_EVENT | CTRL_BREAK_EVENT => {
            if shared.terminate.swap(true, Ordering::SeqCst) {
                // SAFETY: this process's own pseudo-handle. What the user asked for: out, now.
                let _ = unsafe { TerminateProcess(GetCurrentProcess(), SECOND_CTRL_C) };
            }
            shared.waker.wake();
            true.into()
        }
        CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => {
            log::info!("the console is closing; handing the session default back");
            shared.quit();
            if !shared.wait_for_the_way_out(HANDBACK_WAIT) {
                log::warn!("the console closed before FxSound had handed the default device back");
            }
            true.into()
        }
        _ => false.into(),
    }
}

/// The hidden window's procedure: see the module docs.
unsafe extern "system" fn session_window(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: every call below is on `window`, on its own thread, inside its own procedure; the
    // pointer kept in its user data is the `Box<Arc<Shared>>` made for it, freed at
    // `WM_NCDESTROY`, the last message a window gets.
    unsafe {
        match message {
            WM_NCCREATE => {
                let create = &*(lparam.0 as *const CREATESTRUCTW);
                SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize);
                DefWindowProcW(window, message, wparam, lparam)
            }
            // FxSound never holds the session up; it quits when the session does end.
            WM_QUERYENDSESSION => LRESULT(1),
            WM_ENDSESSION => {
                let shared = GetWindowLongPtrW(window, GWLP_USERDATA) as *const Arc<Shared>;
                if wparam.0 != 0
                    && let Some(shared) = shared.as_ref()
                {
                    log::info!("the session is ending; handing the session default back");
                    shared.quit();
                    if !shared.wait_for_the_way_out(HANDBACK_WAIT) {
                        log::warn!(
                            "the session ended before FxSound had handed the default device back"
                        );
                    }
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            WM_NCDESTROY => {
                let shared = SetWindowLongPtrW(window, GWLP_USERDATA, 0) as *mut Arc<Shared>;
                if !shared.is_null() {
                    drop(Box::from_raw(shared));
                }
                DefWindowProcW(window, message, wparam, lparam)
            }
            _ => DefWindowProcW(window, message, wparam, lparam),
        }
    }
}

/// The class of the hidden window, registered once per process.
fn window_class() -> PCWSTR {
    static REGISTERED: OnceLock<()> = OnceLock::new();
    let name = w!("FxSoundSession");
    REGISTERED.get_or_init(|| {
        let class = WNDCLASSW {
            lpfnWndProc: Some(session_window),
            // SAFETY: this module's own handle.
            hInstance: unsafe { GetModuleHandleW(None) }
                .map(Into::into)
                .unwrap_or_default(),
            lpszClassName: name,
            ..WNDCLASSW::default()
        };
        // SAFETY: `class` outlives the call; a class registered twice is refused, harmlessly.
        unsafe { RegisterClassW(&raw const class) };
    });
    name
}

/// The GUI thread's end: [`Termination::close`] once the way out is over.
#[derive(Debug)]
pub struct Termination {
    shared: Arc<Shared>,
    /// The hidden window's thread, and the window as an address — a window handle may be posted
    /// to from any thread, but it is not `Send` as a pointer.
    window: Option<(JoinHandle<()>, isize)>,
}

/// Watch for everything that ends FxSound from outside (see the module docs), raising
/// `terminate` and waking the pump through `waker` for each.
///
/// Never fails: a handler that cannot be installed is a log line, and that way of ending FxSound
/// is then the abrupt one.
#[must_use]
pub fn watch(terminate: Arc<AtomicBool>, waker: &Waker) -> Termination {
    let termination = Termination::start(terminate, waker);
    if CONSOLE.set(Arc::clone(&termination.shared)).is_ok() {
        // SAFETY: a handler with the signature Windows calls, for the life of the process.
        if let Err(err) = unsafe { SetConsoleCtrlHandler(Some(on_console_event), true) } {
            log::warn!("Ctrl+C and the console closing will not quit cleanly: {err}");
        }
    }
    termination
}

impl Termination {
    /// The hidden window alone, without the console's handler, which is the process's for good.
    fn start(terminate: Arc<AtomicBool>, waker: &Waker) -> Self {
        let shared = Arc::new(Shared {
            terminate,
            waker: waker.clone(),
            done: Mutex::new(false),
            settled: Condvar::new(),
        });
        let (tx, rx) = crossbeam_channel::bounded(1);
        let for_window = Arc::clone(&shared);
        let spawned = thread::Builder::new()
            .name("fxsound-session".to_owned())
            .spawn(move || run_window(for_window, &tx));
        let window = match spawned {
            Ok(thread) => match rx.recv() {
                Ok(Some(window)) => Some((thread, window)),
                Ok(None) | Err(_) => {
                    let _ = thread.join();
                    None
                }
            },
            Err(err) => {
                log::warn!("the end of the session will not quit cleanly: {err}");
                None
            }
        };
        Self { shared, window }
    }

    /// The way out has handed the devices back: whatever waits for it may let the process end.
    pub fn close(&self) {
        self.shared.finish();
    }
}

impl Drop for Termination {
    fn drop(&mut self) {
        self.close();
        if let Some((thread, window)) = self.window.take() {
            // SAFETY: posting to a window of ours; a window that is gone already refuses it.
            let posted = unsafe {
                PostMessageW(
                    Some(HWND(window as *mut std::ffi::c_void)),
                    WM_CLOSE,
                    WPARAM(0),
                    LPARAM(0),
                )
            };
            if posted.is_ok() && thread.join().is_err() {
                log::warn!("the session watcher's thread panicked");
            }
        }
    }
}

/// The hidden window's thread: create it, say where it is on `ready`, and pump its messages until
/// it is closed.
fn run_window(shared: Arc<Shared>, ready: &crossbeam_channel::Sender<Option<isize>>) {
    let data = Box::into_raw(Box::new(shared));
    // SAFETY: the class is registered; `data` is handed to the window, which frees it.
    let created = unsafe {
        CreateWindowExW(
            // Never on the taskbar, never shown: a window only so that the session's end is sent
            // to FxSound at all. Top-level, not message-only, for the same reason.
            WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0),
            window_class(),
            w!("FxSound"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            GetModuleHandleW(None).ok().map(Into::into),
            Some(data.cast_const().cast()),
        )
    };
    let window = match created {
        Ok(window) => window,
        Err(err) => {
            // The window never took `data`.
            // SAFETY: made above and handed to nobody.
            drop(unsafe { Box::from_raw(data) });
            log::warn!("the end of the session will not quit cleanly: {err}");
            let _ = ready.send(None);
            return;
        }
    };
    let _ = ready.send(Some(window.0 as isize));
    let mut msg = MSG::default();
    // SAFETY: the loop of the thread the window belongs to.
    unsafe {
        while GetMessageW(&raw mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&raw const msg);
            DispatchMessageW(&raw const msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::SendMessageW;

    fn send(termination: &Termination, message: u32, wparam: usize) -> LRESULT {
        let (_, window) = termination.window.as_ref().expect("a window");
        // SAFETY: a window of ours; `SendMessageW` waits for its procedure to answer.
        unsafe {
            SendMessageW(
                HWND(*window as *mut std::ffi::c_void),
                message,
                Some(WPARAM(wparam)),
                Some(LPARAM(0)),
            )
        }
    }

    #[test]
    fn the_session_may_end_and_its_end_is_the_trays_exit_once_the_devices_are_back() {
        let terminate = Arc::new(AtomicBool::new(false));
        let waker = Waker::new();
        let termination = Arc::new(Termination::start(Arc::clone(&terminate), &waker));
        assert_eq!(send(&termination, WM_QUERYENDSESSION, 0), LRESULT(1));
        assert!(!terminate.load(Ordering::SeqCst), "asking is not ending");

        let ending = {
            let termination = Arc::clone(&termination);
            thread::spawn(move || {
                let started = Instant::now();
                send(&termination, WM_ENDSESSION, 1);
                started.elapsed()
            })
        };
        // The pump is woken and told to quit, and the window holds the session until the way out
        // is over.
        let deadline = Instant::now() + Duration::from_secs(5);
        while !terminate.load(Ordering::SeqCst) {
            assert!(Instant::now() < deadline, "the flag was never raised");
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(waker.pending().len(), 1, "the pump was woken");
        thread::sleep(Duration::from_millis(100));
        assert!(!ending.is_finished(), "the session waits for the hand-back");
        termination.close();
        let waited = ending.join().expect("the sender");
        assert!(waited >= Duration::from_millis(100), "{waited:?}");
        assert!(waited < HANDBACK_WAIT, "{waited:?}");
    }

    #[test]
    fn a_session_end_that_was_called_off_changes_nothing() {
        let terminate = Arc::new(AtomicBool::new(false));
        let termination = Termination::start(Arc::clone(&terminate), &Waker::new());
        assert_eq!(send(&termination, WM_ENDSESSION, 0), LRESULT(0));
        assert!(!terminate.load(Ordering::SeqCst));
        drop(termination);
    }
}
