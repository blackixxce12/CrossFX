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
//!   terminal it was started from to print what its command line has to say
//!   ([`attach_parent_console`]), and lets go of it as soon as it is the running FxSound, before
//!   its engine starts ([`release_borrowed_console`]), as a job sent to the background is out of
//!   the reach of the Ctrl+C typed for the next command, `fxsound --watch` say, and of the
//!   terminal closing. A process attached to a console gets that console's Ctrl+C, Ctrl+Break and
//!   close, and with no handler of its own each one ends it — held over the start, the terminal
//!   would end FxSound in the middle of taking over the default device. What the start has to say
//!   once the engine is up, the report of the options it was started with, is printed on the
//!   terminal borrowed again for as long as the printing takes ([`on_the_terminal`]), and only
//!   when some of it is for the terminal, since no handler covers that borrow whole: one leaves
//!   Ctrl+C and Ctrl+Break to the next command while the report is printed, but it covers neither
//!   the moments of attaching and of letting go — a Ctrl+C on its way then reaches FxSound with no
//!   handler to take it — nor the terminal closing, which ends whatever is attached to it. A plain
//!   `fxsound`, `--show`, `--hide` and a line of settings that all went through have nothing to
//!   report, and a report whose streams were all redirected has nothing for the terminal: both
//!   leave it alone. Every borrow keeps what was redirected where it was sent — `fxsound --status
//!   --json > status.json`, `fxsound --watch | findstr preset`, `fxsound 2> fxsound.log` typed in
//!   cmd.exe — although attaching a console gives the process that console's standard handles in
//!   place of the ones cmd.exe started it with.
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
//!   end. So do the tray's windows (`crate::win::tray`), its own and the icon's, for a closer that
//!   sends every window `WM_CLOSE`. Nothing waits here: the message was posted, and its sender
//!   waits for the process.
//!
//! The pump needs waking for the flag: the handlers do that through the [`Waker`], which is all
//! they do besides the flag and the wait.

use std::cell::Cell;
use std::os::windows::io::{FromRawHandle as _, OwnedHandle};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    GENERIC_READ, GENERIC_WRITE, HANDLE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TYPE_UNKNOWN,
    GetFileType, OPEN_EXISTING,
};
use windows::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, CONSOLE_MODE, CTRL_BREAK_EVENT, CTRL_C_EVENT,
    CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT, FreeConsole, GetConsoleMode,
    GetConsoleWindow, GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE, STD_INPUT_HANDLE,
    STD_OUTPUT_HANDLE, SetConsoleCtrlHandler, SetStdHandle,
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

/// Where this process stands with the terminal it was started from: [`NO_TERMINAL`],
/// [`BORROWED`] or [`GIVEN_BACK`].
static TERMINAL: AtomicU8 = AtomicU8::new(NO_TERMINAL);
/// No terminal was borrowed: there was none — FxSound started from the desktop — or the console
/// is FxSound's own, the debug build's.
const NO_TERMINAL: u8 = 0;
/// [`attach_parent_console`] took the parent's console, which [`release_borrowed_console`] gives
/// back.
const BORROWED: u8 = 1;
/// The borrowed console was given back; [`on_the_terminal`] may borrow it again for a moment, to
/// say something on it.
const GIVEN_BACK: u8 = 2;

/// The window of the console [`attach_parent_console`] took, as an address: the one console
/// [`on_the_terminal`] borrows again. With the shell gone, the parent's process id may name
/// another process by then, and its console is nobody FxSound has anything to say to.
static TERMINAL_WINDOW: AtomicIsize = AtomicIsize::new(0);

/// Which of the standard output and error were the borrowed console's when
/// [`release_borrowed_console`] gave it back: [`STDOUT`], [`STDERR`], both or neither. What is
/// for those is all [`on_the_terminal`] borrows the terminal again for; the others were
/// redirected, and what is for them goes where they were sent.
static CONSOLE_STREAMS: AtomicU8 = AtomicU8::new(0);
/// The standard output, in [`CONSOLE_STREAMS`].
const STDOUT: u8 = 1;
/// The standard error, in [`CONSOLE_STREAMS`].
const STDERR: u8 = 2;

/// The three standard handles, in the order a process has them.
const STANDARD_HANDLES: [STD_HANDLE; 3] = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE];

/// Attach to the parent process's console, for a GUI build started from a terminal: the standard
/// handles that were not redirected then reach that console, so that `fxsound --status` prints
/// where it was typed (`crate::platform::attach_parent_console`), and those that were — to a
/// file, a pipe or `NUL`, `fxsound --status --json > status.json` — stay where they were sent,
/// which the attach alone does not leave them (`keep_redirects`). Fails, harmlessly, for a
/// process that has a console already — the debug build, a console program — or whose parent has
/// none.
pub fn attach_parent_console() {
    let before = standard_handles();
    // SAFETY: a plain call; its failure is the answer "no console to attach to".
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) }.is_ok() {
        keep_redirects(before);
        // SAFETY: a plain query; the handle is kept only as a number to compare.
        TERMINAL_WINDOW.store(unsafe { GetConsoleWindow() }.0 as isize, Ordering::SeqCst);
        TERMINAL.store(BORROWED, Ordering::SeqCst);
    }
}

/// This process's standard handles as they are, in the order of [`STANDARD_HANDLES`]: what
/// [`keep_redirects`] puts back after an attach. One the process has none of is null.
fn standard_handles() -> [HANDLE; 3] {
    // SAFETY: this process's own standard handles, read as numbers.
    STANDARD_HANDLES.map(|which| unsafe { GetStdHandle(which) }.unwrap_or_default())
}

/// Once attached to a console, put back each of the standard handles from `before` the attach
/// ([`standard_handles`]) that was a redirection ([`is_redirected`]).
///
/// Attaching gives a process the console's standard handles: all three, whatever it inherited,
/// when it was started without `STARTF_USESTDHANDLES`, and the ones it has none of when it was
/// started with it. cmd.exe starts a program without the flag, and hands it its redirections —
/// `>`, `2>`, `2>&1`, `|`, `for /f` — as the standard handles it inherits, so the attach took
/// every one of them: from cmd, `fxsound --status --json > status.json` left the file empty and
/// printed on the terminal, and `fxsound 2> fxsound.log` sent the log to the terminal while it
/// was borrowed and nowhere once it was given back. The attach leaves open the handles it takes
/// the place of, and once attached a handle of that console answers as the console's, so what is
/// put back is the process's own redirection and never a console's handle.
fn keep_redirects(before: [HANDLE; 3]) {
    for (which, handle) in STANDARD_HANDLES.into_iter().zip(before) {
        if is_redirected(handle) {
            // SAFETY: a handle this process was started with, and still has open.
            let _ = unsafe { SetStdHandle(which, handle) };
        }
    }
}

/// Whether `handle`, a standard handle from before an attach, was redirected: open, of a kind
/// Windows knows — a file, a pipe, a device such as `NUL` — and not a console's. Asked once
/// attached, when a handle of that console answers as one.
fn is_redirected(handle: HANDLE) -> bool {
    // SAFETY: a plain query on a handle of this process's own; one that is not open has no kind.
    !handle.is_invalid()
        && unsafe { GetFileType(handle) } != FILE_TYPE_UNKNOWN
        && !is_console(handle)
}

/// Give back the console [`attach_parent_console`] took, once this process is the running FxSound
/// and before its engine starts (`crate::platform::release_terminal`): the terminal is the next
/// command's from here on, and neither a Ctrl+C typed there nor its window closing is FxSound's
/// to hear (see the module docs). Which of the standard output and error were the console's is
/// kept, for [`on_the_terminal`]. A no-op for a console FxSound did not borrow — the debug
/// build's own.
pub fn release_borrowed_console() {
    if TERMINAL
        .compare_exchange(BORROWED, GIVEN_BACK, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        CONSOLE_STREAMS.store(let_go_of_the_console(Vec::new()), Ordering::SeqCst);
    }
}

/// Print `stdout` and `stderr`, each unless it is empty, where the standard output and error go
/// once [`release_borrowed_console`] has given the terminal back: a stream that was the
/// terminal's on the terminal, borrowed again for as long as the printing takes
/// (`crate::platform::on_the_terminal`), and one that was redirected —
/// `fxsound --preset=Gamng 2> errors.txt` — where it was sent. How the report of the options
/// FxSound was started with reaches the terminal once the engine is up, the terminal having been
/// given back before it started (see the module docs). Ctrl+C and Ctrl+Break are the next
/// command's while the report is printed, not FxSound's (`the_next_commands`); with nothing in it
/// for the terminal, the terminal is not borrowed at all (`borrows_again`). With no console given
/// back — none was borrowed, or the debug build's own — the report is just printed; and it is
/// printed as well when the terminal is gone, since what goes to a file still goes there.
pub fn on_the_terminal(stdout: &str, stderr: &str) {
    on_the_terminal_with(stdout, stderr, || {
        crate::platform::print_report(stdout, stderr);
    });
}

/// [`on_the_terminal`], with `print` doing the printing of `stdout` and `stderr`, once, on
/// whatever the standard handles are by then: the tests' way of seeing that it was done.
fn on_the_terminal_with(stdout: &str, stderr: &str, print: impl FnOnce()) {
    let streams = CONSOLE_STREAMS.load(Ordering::SeqCst);
    if !borrows_again(TERMINAL.load(Ordering::SeqCst), streams, stdout, stderr) {
        print();
        return;
    }
    let before = standard_handles();
    // SAFETY: a plain call; its failure is the answer "the terminal is gone".
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) }.is_err() {
        print();
        return;
    }
    // First thing once attached, before anything else is asked of the console: every call in
    // between is a moment its Ctrl+C ends FxSound in (`the_next_commands`).
    // SAFETY: a handler with the signature Windows calls, which only answers.
    let held = unsafe { SetConsoleCtrlHandler(Some(the_next_commands), true) }.is_ok();
    keep_redirects(before);
    // SAFETY: a plain query; the handle is only compared.
    let window = unsafe { GetConsoleWindow() }.0 as isize;
    if window == TERMINAL_WINDOW.load(Ordering::SeqCst) {
        let opened = reach_the_console(streams);
        print();
        let_go_of_the_console(opened);
    } else {
        // Another process's console, under the shell's old process id.
        let_go_of_the_console(Vec::new());
        print();
    }
    if held {
        // SAFETY: as above; takes away what was added.
        let _ = unsafe { SetConsoleCtrlHandler(Some(the_next_commands), false) };
    }
}

/// Whether [`on_the_terminal`] borrows the terminal again to print `stdout` and `stderr`: when
/// it was given back ([`GIVEN_BACK`]), and only if there is something to print on a stream that
/// was the terminal's, one of `console_streams` ([`CONSOLE_STREAMS`]). Every borrow is a moment
/// the terminal's Ctrl+C and its closing can end FxSound in ([`the_next_commands`]), so a start
/// with nothing to say there — a plain `fxsound`, `--show`, `--hide`, a line of settings that all
/// went through, or a report only on streams that were redirected, which it reaches without the
/// terminal — leaves the terminal to the next command.
const fn borrows_again(terminal: u8, console_streams: u8, stdout: &str, stderr: &str) -> bool {
    terminal == GIVEN_BACK
        && ((console_streams & STDOUT != 0 && !stdout.is_empty())
            || (console_streams & STDERR != 0 && !stderr.is_empty()))
}

/// While [`on_the_terminal`] borrows the terminal again: Ctrl+C and Ctrl+Break are for whatever
/// the shell runs next, and FxSound, which is not the shell's job, takes them as handled — from
/// the moment this is installed to the moment it is taken away, and no longer. The console sends
/// its events from the moment of the attach, before this can be installed, and one still on its
/// way when the console is let go reaches the default handler once this is gone: at either edge a
/// Ctrl+C ends FxSound as though there were no handler, and no order of the calls closes those
/// edges, which is why nothing is borrowed with nothing to say on the terminal
/// ([`borrows_again`]). The console closing goes on to the next handler: nothing keeps a process
/// attached to a console that closes, whatever its handler answers.
unsafe extern "system" fn the_next_commands(event: u32) -> BOOL {
    matches!(event, CTRL_C_EVENT | CTRL_BREAK_EVENT).into()
}

/// The console's output for each of `streams` ([`CONSOLE_STREAMS`]) that the second attach left
/// with no standard handle. Windows 11 leaves none so, as far as it was seen: the attach gives the
/// console's handles to all three of a process started without `STARTF_USESTDHANDLES`, and to
/// the ones a process started with it has none of — which the streams that were the console's
/// are by then, cleared when it was given back ([`let_go_of_the_console`]). This is for a Windows
/// that does not. The handles opened here are [`let_go_of_the_console`]'s to close.
fn reach_the_console(streams: u8) -> Vec<OwnedHandle> {
    let mut opened = Vec::new();
    for (which, stream) in [(STD_OUTPUT_HANDLE, STDOUT), (STD_ERROR_HANDLE, STDERR)] {
        if streams & stream == 0 {
            continue;
        }
        // SAFETY: this process's own standard handle, and a NUL-terminated name; the handle
        // opened is ours, and kept for closing.
        unsafe {
            if GetStdHandle(which).is_ok_and(|handle| !handle.is_invalid()) {
                continue;
            }
            if let Ok(console) = CreateFileW(
                w!("CONOUT$"),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            ) && SetStdHandle(which, console).is_ok()
            {
                opened.push(OwnedHandle::from_raw_handle(console.0));
            }
        }
    }
    opened
}

/// Detach from the console, clearing the standard handles that were its own first: Windows gives
/// a closed handle's number to the next handle opened, and a late log line written to the old
/// number would land there. A handle that was redirected, `fxsound 2> fxsound.log`, is not the
/// console's — it was put back after the attach ([`keep_redirects`]) — and stays, and so does the
/// log in it. `opened` are the console handles [`reach_the_console`] opened, closed once nothing
/// refers to them. Returns which of the standard output and error were the console's
/// ([`CONSOLE_STREAMS`]).
fn let_go_of_the_console(opened: Vec<OwnedHandle>) -> u8 {
    let mut console_streams = 0;
    for (which, stream) in [
        (STD_INPUT_HANDLE, 0),
        (STD_OUTPUT_HANDLE, STDOUT),
        (STD_ERROR_HANDLE, STDERR),
    ] {
        if clear_if_the_consoles(which) {
            console_streams |= stream;
        }
    }
    // SAFETY: a plain call; nothing of this process refers to the console any more.
    if let Err(err) = unsafe { FreeConsole() } {
        log::debug!("the terminal FxSound was started from is still attached: {err}");
    }
    drop(opened);
    console_streams
}

/// Clear the standard handle `which` if it is a console's. Returns whether it was.
fn clear_if_the_consoles(which: STD_HANDLE) -> bool {
    // SAFETY: this process's own standard handle; one it has none of is no handle at all.
    let Ok(handle) = (unsafe { GetStdHandle(which) }) else {
        return false;
    };
    if !is_console(handle) {
        return false;
    }
    // SAFETY: as above.
    let _ = unsafe { SetStdHandle(which, HANDLE::default()) };
    true
}

/// Whether `handle` is a console's: a handle that is not fails the mode query.
fn is_console(handle: HANDLE) -> bool {
    let mut mode = CONSOLE_MODE::default();
    // SAFETY: a plain query on a handle of this process's own, and an out-pointer to a local.
    unsafe { GetConsoleMode(handle, &raw mut mode) }.is_ok()
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

/// The watcher window's share of [`Shared`] while [`run_window`] makes it: the window takes it out
/// at `WM_NCCREATE` ([`session_window`]), and whatever is still in it when `CreateWindowExW`
/// returns was never the window's.
type Slot = Cell<Option<Arc<Shared>>>;

/// The watcher window's procedure: see the module docs.
unsafe extern "system" fn session_window(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: every call below is on `window`, on its own thread, inside its own procedure. Its
    // user data is null or the `Arc<Shared>` it took from its slot at `WM_NCCREATE`, as
    // `Arc::into_raw` made it, and given back to `Arc::from_raw` once, at `WM_NCDESTROY`: the last
    // message a window gets, which Windows sends a window whose creation fails after
    // `WM_NCCREATE` as well.
    unsafe {
        match message {
            WM_NCCREATE => {
                let create = &*(lparam.0 as *const CREATESTRUCTW);
                // The slot is `run_window`'s, alive for the whole `CreateWindowExW` this message
                // comes inside; from here on the share is the window's, whatever becomes of it.
                if let Some(slot) = create.lpCreateParams.cast::<Slot>().cast_const().as_ref()
                    && let Some(shared) = slot.take()
                {
                    SetWindowLongPtrW(window, GWLP_USERDATA, Arc::into_raw(shared) as isize);
                }
                DefWindowProcW(window, message, wparam, lparam)
            }
            // FxSound never holds the session up; it quits when the session does end.
            WM_QUERYENDSESSION => LRESULT(1),
            WM_ENDSESSION => {
                let shared = GetWindowLongPtrW(window, GWLP_USERDATA) as *const Shared;
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
                let shared = GetWindowLongPtrW(window, GWLP_USERDATA) as *const Shared;
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
                let shared = SetWindowLongPtrW(window, GWLP_USERDATA, 0) as *const Shared;
                if !shared.is_null() {
                    drop(Arc::from_raw(shared));
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
            .spawn(move || run_window(window_class(), for_window, &tx));
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

/// The watcher window's thread: create it, of `class` ([`window_class`]), say where it is on
/// `ready`, and pump its messages until it is closed.
fn run_window(
    class: PCWSTR,
    shared: Arc<Shared>,
    ready: &crossbeam_channel::Sender<Option<isize>>,
) {
    // The window's share, until it takes it ([`Slot`]). A creation that fails after
    // `WM_NCCREATE` has freed it already, by `WM_NCDESTROY`; one that failed before has left it
    // here. Either way it is freed once.
    let slot: Slot = Cell::new(Some(shared));
    // SAFETY: the class is registered; `slot` outlives the call, the only time the window reads it.
    let created = unsafe {
        CreateWindowExW(
            // Top-level, not message-only, so that the session's end is sent to it at all; shown
            // and topmost so that `taskkill` picks it (see the module docs). Nothing of it is ever
            // seen: no size, layered with nothing drawn, click-through, never activated, and a
            // tool window, which no taskbar and no Alt+Tab lists. Not titled "FxSound", which
            // FxSound LLC's FxSound's window is: whatever looks for that one by its title — to
            // close it, say — must not find this.
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST,
            class,
            w!("FxSound Port session"),
            WS_POPUP | WS_VISIBLE,
            0,
            0,
            0,
            0,
            None,
            None,
            GetModuleHandleW(None).ok().map(Into::into),
            Some((&raw const slot).cast()),
        )
    };
    let window = match created {
        Ok(window) => window,
        Err(err) => {
            drop(slot);
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

    fn shared() -> Arc<Shared> {
        Arc::new(Shared {
            terminate: Arc::new(AtomicBool::new(false)),
            waker: Waker::new(),
            done: Mutex::new(false),
            settled: Condvar::new(),
        })
    }

    /// A class whose windows fail to be made once `WM_NCCREATE` has run, as `DefWindowProcW`
    /// makes them fail when the desktop heap has no room left for the title.
    fn refusing_class() -> PCWSTR {
        unsafe extern "system" fn refusing(
            window: HWND,
            message: u32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> LRESULT {
            // SAFETY: as for `session_window`, which this wraps.
            let answer = unsafe { session_window(window, message, wparam, lparam) };
            if message == WM_NCCREATE {
                LRESULT(0)
            } else {
                answer
            }
        }
        static REGISTERED: OnceLock<()> = OnceLock::new();
        let name = w!("FxSoundSessionRefusing");
        REGISTERED.get_or_init(|| {
            let class = WNDCLASSW {
                lpfnWndProc: Some(refusing),
                // SAFETY: this module's own handle.
                hInstance: unsafe { GetModuleHandleW(None) }
                    .map(Into::into)
                    .unwrap_or_default(),
                lpszClassName: name,
                ..WNDCLASSW::default()
            };
            // SAFETY: `class` outlives the call.
            unsafe { RegisterClassW(&raw const class) };
        });
        name
    }

    #[test]
    fn a_window_that_fails_to_be_made_frees_its_share_once_whenever_it_fails() {
        // After `WM_NCCREATE`: the window took its share, and `WM_NCDESTROY` frees it. The share
        // used to be freed a second time on the way out of `run_window`, and the process died of
        // heap corruption instead of logging that the session end would not quit cleanly.
        let after = shared();
        let (tx, rx) = crossbeam_channel::bounded(1);
        run_window(refusing_class(), Arc::clone(&after), &tx);
        assert_eq!(rx.recv(), Ok(None), "no window, and the caller is told");
        assert_eq!(Arc::strong_count(&after), 1, "freed once, by the window");

        // Before it: no window ever had it, and it goes with the slot.
        let before = shared();
        run_window(
            w!("FxSoundSessionNeverRegistered"),
            Arc::clone(&before),
            &tx,
        );
        assert_eq!(rx.recv(), Ok(None));
        assert_eq!(Arc::strong_count(&before), 1, "freed once, by the slot");
    }

    #[test]
    fn with_no_terminal_given_back_the_report_is_printed_where_the_handles_go() {
        // The test process has a console of its own, as the debug build does: nothing was
        // borrowed, so nothing is attached or let go of, and the report is printed all the same —
        // once, whichever stream it is on.
        assert_ne!(TERMINAL.load(Ordering::SeqCst), GIVEN_BACK);
        for (stdout, stderr) in [
            ("status power=true", ""),
            ("", "no preset named \"Gamng\""),
            ("status power=true", "a note"),
        ] {
            let mut printed = 0;
            on_the_terminal_with(stdout, stderr, || printed += 1);
            assert_eq!(printed, 1, "{stdout:?} {stderr:?}");
        }
    }

    #[test]
    fn the_terminal_given_back_is_borrowed_again_only_to_say_something_on_it() {
        const BOTH: u8 = STDOUT | STDERR;
        const STATUS: &str = "status power=true";
        const REFUSAL: &str = "no preset named \"Gamng\"";
        for (terminal, console_streams, stdout, stderr, borrows) in [
            // A plain `fxsound`, `--show`, `--hide`, settings that all went through: nothing to
            // say, and the terminal stays the next command's. Its Ctrl+C at the moment of
            // attaching or of letting go, and its closing while attached, would end FxSound; a
            // start that borrows nothing gives them no moment to.
            (GIVEN_BACK, BOTH, "", "", false),
            // Something on either stream of the terminal's is said where the line was typed.
            (GIVEN_BACK, BOTH, "", REFUSAL, true),
            (GIVEN_BACK, BOTH, STATUS, "", true),
            // A report only on streams that were redirected goes where they were sent, without
            // the terminal: `--preset=Gamng 2> errors.txt`, `--status > status.txt`, and both
            // streams redirected.
            (GIVEN_BACK, STDOUT, "", REFUSAL, false),
            (GIVEN_BACK, STDERR, STATUS, "", false),
            (GIVEN_BACK, 0, STATUS, REFUSAL, false),
            // A report on a redirected stream and on the terminal's: borrowed, for the terminal's.
            (GIVEN_BACK, STDERR, STATUS, REFUSAL, true),
            (GIVEN_BACK, STDOUT, STATUS, REFUSAL, true),
            // Nothing given back, nothing to borrow again: the console is FxSound's own, or still
            // the borrowed one, and the report goes where the handles go.
            (NO_TERMINAL, BOTH, STATUS, REFUSAL, false),
            (BORROWED, BOTH, STATUS, REFUSAL, false),
        ] {
            assert_eq!(
                borrows_again(terminal, console_streams, stdout, stderr),
                borrows,
                "terminal {terminal}, console streams {console_streams}, {stdout:?}, {stderr:?}"
            );
        }
    }

    #[test]
    fn a_file_a_pipe_and_nul_are_redirections_and_a_console_or_no_handle_is_not() {
        let handle = |raw: &dyn std::os::windows::io::AsRawHandle| HANDLE(raw.as_raw_handle());
        let dir = tempfile::tempdir().expect("temp dir");
        let file = std::fs::File::create(dir.path().join("status.json")).expect("a file");
        assert!(is_redirected(handle(&file)), "`> status.json`");
        let (reader, writer) = std::io::pipe().expect("a pipe");
        assert!(is_redirected(handle(&writer)), "`| findstr preset`");
        assert!(is_redirected(handle(&reader)));
        let nul = std::fs::OpenOptions::new()
            .write(true)
            .open("NUL")
            .expect("NUL");
        assert!(is_redirected(handle(&nul)), "`> nul`");
        // What a process has where it has no standard handle: put back, it would be nothing.
        assert!(!is_redirected(HANDLE::default()));
        assert!(!is_redirected(HANDLE(-1_isize as *mut std::ffi::c_void)));
        // A console's handle is the console's, never a redirection to keep — where the test
        // process has a console to open one of.
        if let Ok(console) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("CONOUT$")
        {
            assert!(!is_redirected(handle(&console)));
        }
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
