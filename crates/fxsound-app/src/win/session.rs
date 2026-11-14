//! Ending FxSound the way the tray's Exit does, whatever Windows ends it with.
//!
//! On Linux, `SIGTERM`, `SIGINT` and `SIGHUP` raise a flag that the pump reads as the tray's Exit
//! (`main.rs`): the engine hands the default devices back and stops, and only then does the
//! process end. A process that just dies leaves the system default on FxSound's virtual device,
//! with nothing behind it to play the sound on — so the Windows build needs the same for each way
//! Windows ends a program:
//!
//! - **Ctrl+C and Ctrl+Break** in the console FxSound runs in — the debug build's, a console
//!   program the shell waits for: the flag, as `SIGINT` raises it. A second one while the way out
//!   is under way ends the process at once with the conventional 130, as a second Ctrl+C does on
//!   Linux. A release build is a GUI program, which the shell does not wait for: it borrows the
//!   terminal it was started from to print what its command line has to say, and lets go of it
//!   once it is the running FxSound ([`release_borrowed_console`]), as a job sent to the
//!   background is out of the reach of the Ctrl+C typed for the next command, `fxsound --watch`
//!   say.
//! - **The console closing**, and log-off and shutdown for a process the system tells through its
//!   console (`CTRL_CLOSE_EVENT`, `CTRL_LOGOFF_EVENT`, `CTRL_SHUTDOWN_EVENT`): the flag, and then
//!   the handler waits — the process ends as soon as it returns — until the way out has handed
//!   the devices back ([`Termination::close`]), or [`HANDBACK_WAIT`] at most.
//! - **Log-off, shutdown and a Restart Manager close** for a program with windows, which hears
//!   them as `WM_QUERYENDSESSION` and `WM_ENDSESSION` and not through its console at all: a window
//!   on a thread of its own, `fxsound-session`, answers the first with "go ahead" — a sound
//!   program has no business holding the session up — and does for the second what the console's
//!   close does.
//! - **`taskkill` without `/F`**, `kill` on Windows: it posts `WM_CLOSE` to one window of the
//!   process, the first visible one in Z-order — or, with none visible, the first of all. That
//!   window is this one: shown, but drawing nothing — no size, layered, click-through, never
//!   activated, on no taskbar — and topmost, which puts it ahead of FxSound's own window, whose
//!   close only sends FxSound to the tray as Alt+F4 does, and of the hidden windows of winit and
//!   of the OpenGL driver, which would be destroyed under them (seen: three `taskkill`s took
//!   those three away one by one, and FxSound ran on). It hears the close as the flag, as
//!   `SIGTERM` raises it, and stays: gone, it would leave FxSound with nothing to hear the session
//!   end. So does the tray's own window (`crate::win::tray`), for a closer that sends every window
//!   `WM_CLOSE`. Nothing waits here: the message was posted, and its sender waits for the process.
//!
//! The pump needs waking for the flag: the handlers do that through the [`Waker`], which is all
//! they do besides the flag and the wait.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, CONSOLE_MODE, CTRL_BREAK_EVENT, CTRL_C_EVENT,
    CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT, FreeConsole, GetConsoleMode,
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleCtrlHandler,
    SetStdHandle,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
    GetMessageW, GetWindowLongPtrW, MSG, PostMessageW, PostQuitMessage, RegisterClassW,
    SetWindowLongPtrW, TranslateMessage, WM_APP, WM_CLOSE, WM_DESTROY, WM_ENDSESSION, WM_NCCREATE,
    WM_NCDESTROY, WM_QUERYENDSESSION, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP, WS_VISIBLE,
};
use windows::core::{BOOL, PCWSTR, w};

use crate::wake::Waker;

/// How long a session that is ending, or a console that is closing, waits for the devices to be
/// handed back. Windows gives a program about five seconds before it offers to end it by force,
/// and the hand-back takes a few milliseconds; the rest is for a GUI thread that was busy.
pub const HANDBACK_WAIT: Duration = Duration::from_secs(4);

/// Posted to the watcher window by [`Termination`]'s drop: the watch is over, the window goes and
/// its thread ends. A message of FxSound's own, since `WM_CLOSE` is a quit from outside (see the
/// module docs).
const WM_STOP_WATCHING: u32 = WM_APP + 1;

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

/// [`attach_parent_console`] took the parent's console, which [`release_borrowed_console`] gives
/// back.
static BORROWED_CONSOLE: AtomicBool = AtomicBool::new(false);

/// Attach to the parent process's console, for a GUI build started from a terminal: the standard
/// handles that were not redirected then reach that console, so that `fxsound --status` prints
/// where it was typed (`crate::platform::attach_parent_console`). Fails, harmlessly, for a
/// process that has a console already — the debug build, a console program — or whose parent has
/// none.
pub fn attach_parent_console() {
    // SAFETY: a plain call; its failure is the answer "no console to attach to".
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) }.is_ok() {
        BORROWED_CONSOLE.store(true, Ordering::SeqCst);
    }
}

/// Give back the console [`attach_parent_console`] took, once this process is the running FxSound
/// and has printed what its command line had to say (`crate::platform::release_terminal`): the
/// terminal is the next command's from here on, and neither a Ctrl+C typed there nor its window
/// closing is FxSound's to hear (see the module docs). A no-op for a console FxSound did not
/// borrow — the debug build's own.
///
/// The standard handles that were the console's are cleared before it goes: Windows gives a
/// closed handle's number to the next handle opened, and a late log line written to the old
/// number would land there. A handle that was redirected, `fxsound 2> fxsound.log`, stays, and so
/// does the log in it.
pub fn release_borrowed_console() {
    if !BORROWED_CONSOLE.swap(false, Ordering::SeqCst) {
        return;
    }
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        let mut mode = CONSOLE_MODE::default();
        // SAFETY: this process's own standard handles, and an out-pointer to a local. A handle
        // that is not a console's fails the mode query and is left as it is.
        unsafe {
            if let Ok(handle) = GetStdHandle(which)
                && !handle.is_invalid()
                && GetConsoleMode(handle, &raw mut mode).is_ok()
            {
                let _ = SetStdHandle(which, HANDLE::default());
            }
        }
    }
    // SAFETY: a plain call; nothing of this process refers to the console any more.
    if let Err(err) = unsafe { FreeConsole() } {
        log::debug!("the terminal FxSound was started from is still attached: {err}");
    }
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

/// The watcher window's procedure: see the module docs.
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
            // `taskkill`, or an installer closing FxSound: the tray's Exit, and the window stays.
            WM_CLOSE => {
                let shared = GetWindowLongPtrW(window, GWLP_USERDATA) as *const Arc<Shared>;
                if let Some(shared) = shared.as_ref() {
                    log::info!("asked to close from outside; quitting as the tray's Exit does");
                    shared.quit();
                }
                LRESULT(0)
            }
            WM_STOP_WATCHING => {
                let _ = DestroyWindow(window);
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

/// The class of the watcher window, registered once per process.
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
    /// The watcher window's thread, and the window as an address — a window handle may be posted
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
    /// The watcher window alone, without the console's handler, which is the process's for good.
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
                    WM_STOP_WATCHING,
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

/// The watcher window's thread: create it, say where it is on `ready`, and pump its messages until
/// it is closed.
fn run_window(shared: Arc<Shared>, ready: &crossbeam_channel::Sender<Option<isize>>) {
    let data = Box::into_raw(Box::new(shared));
    // SAFETY: the class is registered; `data` is handed to the window, which frees it.
    let created = unsafe {
        CreateWindowExW(
            // Top-level, not message-only, so that the session's end is sent to it at all; shown
            // and topmost so that `taskkill` picks it (see the module docs). Nothing of it is ever
            // seen: no size, layered with nothing drawn, click-through, never activated, and a
            // tool window, which no taskbar and no Alt+Tab lists. Not titled "FxSound", which
            // FxSound LLC's FxSound's window is: whatever looks for that one by its title — to
            // close it, say — must not find this.
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST,
            window_class(),
            w!("FxSound Port session"),
            WS_POPUP | WS_VISIBLE,
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

    #[test]
    fn a_close_from_outside_is_the_trays_exit_and_the_watcher_stays_until_it_is_dropped() {
        use windows::Win32::UI::WindowsAndMessaging::{
            GWL_EXSTYLE, GetWindowLongPtrW, GetWindowRect, IsWindow, IsWindowVisible,
            WINDOW_EX_STYLE,
        };
        let terminate = Arc::new(AtomicBool::new(false));
        let waker = Waker::new();
        let termination = Termination::start(Arc::clone(&terminate), &waker);
        let (_, window) = termination.window.as_ref().expect("a window");
        let window = HWND(*window as *mut std::ffi::c_void);
        // What `taskkill` looks for: the first window of the process that is shown, which a
        // topmost one is. Nothing of it can be seen or clicked.
        let mut rect = windows::Win32::Foundation::RECT::default();
        // SAFETY: queries on a window of ours, and an out-pointer to a local.
        let (visible, extended) = unsafe {
            GetWindowRect(window, &raw mut rect).expect("its rectangle");
            (
                IsWindowVisible(window).as_bool(),
                WINDOW_EX_STYLE(GetWindowLongPtrW(window, GWL_EXSTYLE) as u32),
            )
        };
        assert!(visible, "taskkill passes over a window that is not shown");
        for style in [
            WS_EX_TOPMOST,
            WS_EX_LAYERED,
            WS_EX_TRANSPARENT,
            WS_EX_NOACTIVATE,
        ] {
            assert!(extended.contains(style), "{extended:?} lacks {style:?}");
        }
        assert_eq!((rect.right - rect.left, rect.bottom - rect.top), (0, 0));

        // What `taskkill` without `/F` sends it.
        assert_eq!(send(&termination, WM_CLOSE, 0), LRESULT(0));
        assert!(terminate.load(Ordering::SeqCst), "a close is a quit");
        assert_eq!(waker.pending().len(), 1, "the pump was woken");
        // SAFETY: a query on a handle of ours.
        assert!(
            unsafe { IsWindow(Some(window)) }.as_bool(),
            "the window still hears the session end"
        );
        // Its own way out ends the thread, which the drop joins.
        drop(termination);
        // SAFETY: as above; the handle is not used for anything but the question.
        assert!(!unsafe { IsWindow(Some(window)) }.as_bool());
    }
}
