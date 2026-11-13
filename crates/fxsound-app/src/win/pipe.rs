//! The control pipe: `\\.\pipe\fxsound-<user SID>`, the single-instance lock and the command
//! channel of the Windows build, where Linux has a Unix socket and its lock file (`crate::ipc`).
//!
//! [`PipeListener`] and [`PipeStream`] have the methods of `UnixListener` and `UnixStream` that
//! `crate::ipc` is written against — `connect`, the read and write timeouts, a non-blocking read,
//! `incoming` — so that the protocol there is one piece of code on both systems: one JSON line
//! per request and per reply, `--watch` streams, the connection limit, the deadlines.
//!
//! # The lock
//!
//! The first instance of a pipe name is created with `FILE_FLAG_FIRST_PIPE_INSTANCE`, which fails
//! while any instance of that name exists — in any process. Taking it is what makes a process the
//! primary, as taking the `flock` does on Linux, and the kernel gives it up with the last handle,
//! so a crash leaves nothing stale behind. Every instance after the first is created *before* the
//! one before it is handed to its caller ([`PipeListener::accept`]): a name with no instance left
//! would be free for a second FxSound to take while the first one is still running.
//!
//! # Who may connect
//!
//! The pipe's security descriptor grants access to this user's SID alone, and remote clients are
//! refused (`PIPE_REJECT_REMOTE_CLIENTS`). The name itself protects nothing — pipe names are one
//! namespace for the whole machine — so a client checks the owner of the pipe it reached against
//! its own SID before it writes a byte, and connects at identification level, so that a pipe
//! another account created first cannot act as the caller either.
//!
//! # Timeouts
//!
//! A pipe handle has no receive or send timeout. Every handle here is opened for overlapped I/O,
//! and each read or write waits for its own completion for as long as the timeout allows, and is
//! cancelled — and waited for again, so that the buffer it was given is not written into after it
//! returns — when the timeout passes.

use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_PIPE_BUSY,
    ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, GENERIC_READ, GENERIC_WRITE, HANDLE,
    INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WIN32_ERROR,
};
use windows::Win32::Security::SECURITY_ATTRIBUTES;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE,
    OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
    WriteFile,
};
use windows::Win32::System::IO::{
    CancelIoEx, GetOverlappedResult, GetOverlappedResultEx, OVERLAPPED,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeServerProcessId, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, PeekNamedPipe,
    WaitNamedPipeW,
};
use windows::Win32::System::Threading::{INFINITE, WaitForMultipleObjects};
use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
use windows::core::PCWSTR;

use super::user::{self, SecurityDescriptor};
use super::{Event, io_error, is, millis, wide};

/// Where every pipe's name starts.
pub const PIPE_PREFIX: &str = r"\\.\pipe\";

/// The size asked for each direction's buffer: a request is at most `ipc::MAX_REQUEST_BYTES`, and
/// a `--watch` stream gets many events' worth of room before a reader that stopped reading is
/// dropped for it.
const BUFFER_BYTES: u32 = 64 * 1024;

/// How long a client waits for an instance of a pipe whose every instance is taken — the moment
/// between a connection and the primary's next instance, under a burst of keybinds.
const BUSY_WAIT: Duration = Duration::from_secs(2);

/// What waiting out `GetOverlappedResultEx` sets: `WAIT_TIMEOUT`, as a Win32 error.
const WAIT_TIMED_OUT: WIN32_ERROR = WIN32_ERROR(258);

/// The SID this process runs as, read once.
fn own_sid() -> io::Result<&'static str> {
    static SID: OnceLock<String> = OnceLock::new();
    if let Some(sid) = SID.get() {
        return Ok(sid);
    }
    let sid = user::sid()?;
    Ok(SID.get_or_init(|| sid))
}

/// `\\.\pipe\fxsound-<user SID>`: the control pipe of this user's FxSound.
///
/// # Errors
///
/// If the user's SID cannot be read from the process token.
pub fn default_path() -> io::Result<PathBuf> {
    Ok(PathBuf::from(format!(
        "{PIPE_PREFIX}fxsound-{}",
        own_sid()?
    )))
}

/// The control pipe of an FxSound told to keep its instance in `dir` (`ipc::Instance::acquire_in`):
/// the user's pipe name with a hash of the directory after it, so that two directories are two
/// instances, as two runtime directories are on Linux. The tests run their primaries this way.
///
/// # Errors
///
/// As [`default_path`].
pub fn path_in(dir: &Path) -> io::Result<PathBuf> {
    // FNV-1a over the path's UTF-16: stable across runs, which `DefaultHasher` does not promise.
    let hash = wide(dir)
        .iter()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, unit| {
            (hash ^ u64::from(*unit)).wrapping_mul(0x0000_0100_0000_01b3)
        });
    Ok(PathBuf::from(format!(
        "{PIPE_PREFIX}fxsound-{}-{hash:016x}",
        own_sid()?
    )))
}

/// Whether `path` names a pipe rather than a file: `connect` must never open a file someone gave
/// it for a socket, and write a command line into it.
#[must_use]
pub fn is_pipe_path(path: &Path) -> bool {
    path.to_str().is_some_and(|path| {
        path.len() > PIPE_PREFIX.len()
            && path
                .get(..PIPE_PREFIX.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(PIPE_PREFIX))
    })
}

/// Whether a pipe of `path`'s name exists right now, waiting for no instance to come free. For
/// `--self-test`, which must not connect to a running FxSound: a connection is a caller it would
/// answer.
#[must_use]
pub fn exists(path: &Path) -> bool {
    let name = wide(path);
    // SAFETY: `name` is NUL-terminated and outlives the call. Waiting a millisecond is enough to
    // tell "no such pipe" (`ERROR_FILE_NOT_FOUND`) from "there, and every instance busy".
    let free = unsafe { WaitNamedPipeW(PCWSTR(name.as_ptr()), 1) }.as_bool();
    free || io::Error::last_os_error().raw_os_error() != Some(2)
}

/// Create an instance of the pipe `name`, open to this user only.
fn create_instance(
    name: &[u16],
    security: &SecurityDescriptor,
    first: bool,
) -> io::Result<OwnedHandle> {
    let attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(u32::MAX),
        lpSecurityDescriptor: security.as_ptr(),
        bInheritHandle: false.into(),
    };
    let mut open_mode = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
    if first {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    // SAFETY: `name` is NUL-terminated, and it and `attributes` outlive the call.
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            BUFFER_BYTES,
            BUFFER_BYTES,
            0,
            Some(&raw const attributes),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_invalid() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a fresh handle nothing else owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle.0) })
}

/// Whether `e` says the other end of the pipe has gone.
fn other_end_gone(e: &io::Error) -> bool {
    [ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_NOT_CONNECTED]
        .iter()
        .any(|code| e.raw_os_error() == Some(code.0.cast_signed()))
}

/// Run one overlapped operation on `handle` — `start` begins it with the `OVERLAPPED` it is given
/// — and wait up to `timeout` for it. Returns the bytes it moved.
///
/// Cancelled when the timeout passes, and waited for until the cancellation has landed: the
/// operation holds a pointer to the caller's buffer and to the `OVERLAPPED` here, and must be over
/// before either goes. One that completed meanwhile counts as done.
fn overlapped(
    handle: HANDLE,
    timeout: Option<Duration>,
    start: impl FnOnce(*mut OVERLAPPED) -> windows::core::Result<()>,
) -> io::Result<u32> {
    let done = Event::new(true)?;
    let mut operation = OVERLAPPED {
        hEvent: done.raw(),
        ..OVERLAPPED::default()
    };
    if let Err(e) = start(&raw mut operation)
        && !is(&e, ERROR_IO_PENDING)
    {
        return Err(io_error(&e));
    }
    let mut moved = 0_u32;
    // SAFETY: `operation` and the caller's buffer outlive the operation: every path out of this
    // function has seen it complete, fail or be cancelled.
    match unsafe {
        GetOverlappedResultEx(
            handle,
            &raw const operation,
            &raw mut moved,
            millis(timeout),
            false,
        )
    } {
        Ok(()) => Ok(moved),
        Err(e) if is(&e, WAIT_TIMED_OUT) => {
            // SAFETY: as above; the cancelled operation is waited for before returning.
            unsafe {
                let _ = CancelIoEx(handle, Some(&raw const operation));
                match GetOverlappedResult(handle, &raw const operation, &raw mut moved, true) {
                    Ok(()) => Ok(moved),
                    Err(_) => Err(io::Error::from(io::ErrorKind::TimedOut)),
                }
            }
        }
        Err(e) => Err(io_error(&e)),
    }
}

/// A timeout as the atomics below keep it: milliseconds, or `u64::MAX` for none.
fn stored(timeout: Option<Duration>) -> io::Result<u64> {
    match timeout {
        // As `UnixStream` refuses it: a zero timeout would be a poll that never waits.
        Some(timeout) if timeout.is_zero() => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a zero timeout is not a timeout",
        )),
        Some(timeout) => Ok(u64::try_from(timeout.as_millis())
            .unwrap_or(u64::MAX - 1)
            .clamp(1, u64::MAX - 1)),
        None => Ok(u64::MAX),
    }
}

fn loaded(timeout: &AtomicU64) -> Option<Duration> {
    match timeout.load(Ordering::Relaxed) {
        u64::MAX => None,
        ms => Some(Duration::from_millis(ms)),
    }
}

/// One end of a connection on the control pipe: the client's, from [`PipeStream::connect`], or
/// the server's, from [`PipeListener::accept`]. Closing it — dropping it — ends the stream for the
/// other end once that end has read what was written.
#[derive(Debug)]
pub struct PipeStream {
    handle: OwnedHandle,
    read_timeout: AtomicU64,
    write_timeout: AtomicU64,
    nonblocking: AtomicBool,
}

impl PipeStream {
    fn new(handle: OwnedHandle) -> Self {
        Self {
            handle,
            read_timeout: AtomicU64::new(u64::MAX),
            write_timeout: AtomicU64::new(u64::MAX),
            nonblocking: AtomicBool::new(false),
        }
    }

    fn raw(&self) -> HANDLE {
        HANDLE(self.handle.as_raw_handle())
    }

    /// Connect to the pipe at `path` and check that it is this user's.
    ///
    /// # Errors
    ///
    /// `NotFound` when `path` is not a pipe's name or no pipe of that name exists — FxSound is
    /// not running; `PermissionDenied` when the pipe is another account's; any other failure to
    /// open it.
    pub fn connect(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        if !is_pipe_path(path) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} is not the name of a pipe", path.display()),
            ));
        }
        let name = wide(path);
        let until = Instant::now() + BUSY_WAIT;
        let handle = loop {
            // SAFETY: `name` is NUL-terminated and outlives the call; the handle is ours.
            let opened = unsafe {
                CreateFileW(
                    PCWSTR(name.as_ptr()),
                    (GENERIC_READ | GENERIC_WRITE).0,
                    FILE_SHARE_NONE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                    None,
                )
            };
            match opened {
                Ok(handle) => break handle,
                Err(e) if is(&e, ERROR_PIPE_BUSY) && Instant::now() < until => {
                    let left = until.saturating_duration_since(Instant::now());
                    // SAFETY: as above. Whether an instance came free is the next open's to find.
                    let _ = unsafe { WaitNamedPipeW(PCWSTR(name.as_ptr()), millis(Some(left))) };
                }
                Err(e) => return Err(io_error(&e)),
            }
        };
        // SAFETY: a fresh handle nothing else owns.
        let stream = Self::new(unsafe { OwnedHandle::from_raw_handle(handle.0) });
        let owner = user::owner_of(stream.raw())?;
        if owner != own_sid()? {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "{} belongs to another account ({owner}); FxSound does not talk to it",
                    path.display()
                ),
            ));
        }
        Ok(stream)
    }

    /// Let the FxSound at the other end of this client's connection bring a window to the front
    /// once: the one a forwarded `fxsound` or `fxsound --show` opens or raises, which Windows
    /// would otherwise leave behind the window the user is in, with its taskbar button flashing.
    /// Windows gives that right to the process the user is working in — a command just typed or
    /// a shortcut just clicked — and this hands it on. Nothing happens when this process has no
    /// such right to give.
    pub fn let_the_other_end_take_the_foreground(&self) {
        let mut server = 0_u32;
        // SAFETY: this stream's own handle, and an out-pointer to a local.
        if unsafe { GetNamedPipeServerProcessId(self.raw(), &raw mut server) }.is_ok()
            && server != 0
        {
            // SAFETY: a plain call; a refusal is the "no right to give" above.
            let _ = unsafe { AllowSetForegroundWindow(server) };
        }
    }

    /// Both ends of a fresh connection on a pipe of its own, for the tests' streams that need no
    /// listener: `UnixStream::pair`.
    ///
    /// # Errors
    ///
    /// If the pipe cannot be created or connected to.
    #[cfg(test)]
    pub fn pair() -> io::Result<(Self, Self)> {
        use std::sync::atomic::AtomicUsize;
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = PathBuf::from(format!(
            "{PIPE_PREFIX}fxsound-{}-pair-{}-{}",
            own_sid()?,
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let listener = PipeListener::bind(&path)?;
        let client = Self::connect(&path)?;
        let (server, ()) = listener.accept()?;
        Ok((server, client))
    }

    /// How long a read waits; `None` for as long as it takes.
    ///
    /// # Errors
    ///
    /// For `Some(Duration::ZERO)`, as `UnixStream::set_read_timeout`.
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.read_timeout.store(stored(timeout)?, Ordering::Relaxed);
        Ok(())
    }

    /// How long a write waits for room in the pipe; `None` for as long as it takes.
    ///
    /// # Errors
    ///
    /// For `Some(Duration::ZERO)`, as `UnixStream::set_write_timeout`.
    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.write_timeout
            .store(stored(timeout)?, Ordering::Relaxed);
        Ok(())
    }

    /// In non-blocking mode a read with nothing to read is `WouldBlock`, and one after the other
    /// end has gone reads nothing — what `ipc`'s check for a watcher that hung up looks for.
    ///
    /// # Errors
    ///
    /// Never; the `Result` is `UnixStream`'s.
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.nonblocking.store(nonblocking, Ordering::Relaxed);
        Ok(())
    }

    /// A pipe cannot be half closed: the stream ends for the other end when this one is dropped,
    /// after that end has read what was written — which is what `ipc` drops it for straight after
    /// this. `DisconnectNamedPipe` would end it sooner, and throw away what the other end had not
    /// read yet, the `quit` event of a `--watch` stream among it.
    ///
    /// # Errors
    ///
    /// Never; the `Result` is `UnixStream`'s.
    pub const fn shutdown(&self, _how: Shutdown) -> io::Result<()> {
        Ok(())
    }

    fn read_into(&self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let handle = self.raw();
        if self.nonblocking.load(Ordering::Relaxed) {
            let mut available = 0_u32;
            // SAFETY: a peek with no buffer, and an out-pointer to a local.
            let peeked =
                unsafe { PeekNamedPipe(handle, None, 0, None, Some(&raw mut available), None) };
            match peeked.map_err(|e| io_error(&e)) {
                Ok(()) if available == 0 => return Err(io::ErrorKind::WouldBlock.into()),
                Ok(()) => {}
                Err(e) if other_end_gone(&e) => return Ok(0),
                Err(e) => return Err(e),
            }
        }
        let len = buf.len().min(u32::MAX as usize);
        let buf = &mut buf[..len];
        // SAFETY: `buf` is borrowed for the whole of `overlapped`, which outlasts the read.
        let read = overlapped(handle, loaded(&self.read_timeout), |operation| unsafe {
            ReadFile(handle, Some(buf), None, Some(operation))
        });
        match read {
            Ok(read) => Ok(read as usize),
            // The other end closed: end of stream, as a socket reads it.
            Err(e) if other_end_gone(&e) => Ok(0),
            Err(e) => Err(e),
        }
    }

    fn write_from(&self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let handle = self.raw();
        let len = buf.len().min(u32::MAX as usize);
        let buf = &buf[..len];
        // SAFETY: `buf` is borrowed for the whole of `overlapped`, which outlasts the write.
        let written = overlapped(handle, loaded(&self.write_timeout), |operation| unsafe {
            WriteFile(handle, Some(buf), None, Some(operation))
        });
        match written {
            Ok(written) => Ok(written as usize),
            // What a socket says when the other end has gone, which `ipc` looks for to read the
            // reason it was turned away instead.
            Err(e) if other_end_gone(&e) => Err(io::Error::new(io::ErrorKind::BrokenPipe, e)),
            Err(e) => Err(e),
        }
    }
}

impl Read for &PipeStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_into(buf)
    }
}

impl Read for PipeStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_into(buf)
    }
}

impl Write for &PipeStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_from(buf)
    }

    /// Nothing is held back to flush: `FlushFileBuffers` on a pipe waits for the other end to
    /// read everything, which a stuck reader never does.
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Write for PipeStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_from(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Wakes an [`PipeListener::accept`] that is waiting, from another thread, and makes every later
/// one fail at once: the way out of `ipc`'s accept loop.
#[derive(Debug, Clone)]
pub struct Stopper(Arc<Event>);

impl Stopper {
    pub fn stop(&self) {
        self.0.set();
    }
}

/// The server end of the control pipe: the lock, and the instances callers connect to.
#[derive(Debug)]
pub struct PipeListener {
    path: PathBuf,
    name: Vec<u16>,
    security: SecurityDescriptor,
    /// The instance the next caller connects to.
    waiting: Mutex<Option<OwnedHandle>>,
    stop: Arc<Event>,
}

impl PipeListener {
    /// Take `path`'s pipe as its first instance: `Some` when this process is the primary, `None`
    /// when an instance exists already — another FxSound of this user, or, behind a pipe of the
    /// same name, an account whose pipe a client then refuses to talk to.
    ///
    /// # Errors
    ///
    /// If the user's SID or the security descriptor cannot be had, or the pipe cannot be created
    /// for any other reason.
    pub fn bind_first(path: &Path) -> io::Result<Option<Self>> {
        let security = SecurityDescriptor::only(own_sid()?)?;
        let name = wide(path);
        match create_instance(&name, &security, true) {
            Ok(first) => Ok(Some(Self {
                path: path.to_path_buf(),
                name,
                security,
                waiting: Mutex::new(Some(first)),
                stop: Arc::new(Event::new(true)?),
            })),
            Err(e)
                if [ERROR_ACCESS_DENIED, ERROR_PIPE_BUSY]
                    .iter()
                    .any(|code| e.raw_os_error() == Some(code.0.cast_signed())) =>
            {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// [`PipeListener::bind_first`], failing with `AddrInUse` when the pipe exists, as
    /// `UnixListener::bind` does for a socket path that is taken.
    ///
    /// # Errors
    ///
    /// As [`PipeListener::bind_first`], and `AddrInUse`.
    pub fn bind(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        Self::bind_first(path)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("{} exists already", path.display()),
            )
        })
    }

    /// The pipe's name.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What stops [`PipeListener::accept`] from another thread.
    #[must_use]
    pub fn stopper(&self) -> Stopper {
        Stopper(Arc::clone(&self.stop))
    }

    /// Only blocking accepts are offered; `ipc` asks for nothing else.
    ///
    /// # Errors
    ///
    /// `Unsupported` for non-blocking mode.
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        if nonblocking {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "the control pipe only accepts in blocking mode",
            ));
        }
        Ok(())
    }

    fn stopped(&self) -> io::Error {
        io::Error::new(
            io::ErrorKind::Interrupted,
            "the control pipe stopped accepting",
        )
    }

    /// An instance for the next caller, unless one is waiting already.
    fn instance_for_the_next_caller(&self) -> io::Result<()> {
        let mut waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        if waiting.is_none() {
            *waiting = Some(create_instance(&self.name, &self.security, false)?);
        }
        Ok(())
    }

    /// Wait for the next caller and hand its connection over.
    ///
    /// # Errors
    ///
    /// `Interrupted` once stopped ([`PipeListener::stopper`]); any failure to create an instance
    /// or to connect one. An instance that failed is replaced before it is let go.
    pub fn accept(&self) -> io::Result<(PipeStream, ())> {
        // SAFETY: a poll of an event we own.
        if unsafe { WaitForMultipleObjects(&[self.stop.raw()], false, 0) } == WAIT_OBJECT_0 {
            return Err(self.stopped());
        }
        self.instance_for_the_next_caller()?;
        let Some(instance) = self
            .waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        else {
            return Err(io::Error::other(
                "no instance of the control pipe to wait on",
            ));
        };
        let connected = self.connect_instance(&instance);
        // Whatever became of this instance, the name gets the next one before it is let go.
        if let Err(e) = self.instance_for_the_next_caller() {
            log::warn!("the control pipe has no instance for the next caller yet: {e}");
        }
        match connected {
            Ok(()) => Ok((PipeStream::new(instance), ())),
            Err(e) => Err(e),
        }
    }

    /// Wait until a caller has connected to `instance`, or until stopped.
    fn connect_instance(&self, instance: &OwnedHandle) -> io::Result<()> {
        let handle = HANDLE(instance.as_raw_handle());
        let arrived = Event::new(true)?;
        let mut operation = OVERLAPPED {
            hEvent: arrived.raw(),
            ..OVERLAPPED::default()
        };
        // SAFETY: `operation` outlives the connect: it completes, or is cancelled and waited for,
        // before this function returns.
        match unsafe { ConnectNamedPipe(handle, Some(&raw mut operation)) } {
            Ok(()) => return Ok(()),
            // The caller came before the wait did.
            Err(e) if is(&e, ERROR_PIPE_CONNECTED) => return Ok(()),
            Err(e) if is(&e, ERROR_IO_PENDING) => {}
            Err(e) => return Err(io_error(&e)),
        }
        // SAFETY: two events we own.
        let woke =
            unsafe { WaitForMultipleObjects(&[arrived.raw(), self.stop.raw()], false, INFINITE) };
        let mut ignored = 0_u32;
        if woke == WAIT_OBJECT_0 {
            // SAFETY: the completed operation's result, from the `OVERLAPPED` it completed in.
            return unsafe {
                GetOverlappedResult(handle, &raw const operation, &raw mut ignored, false)
            }
            .map_err(|e| io_error(&e));
        }
        // Stopped, or the wait itself failed: cancel, and wait for the cancellation to land.
        // SAFETY: as above.
        unsafe {
            let _ = CancelIoEx(handle, Some(&raw const operation));
            let _ = GetOverlappedResult(handle, &raw const operation, &raw mut ignored, true);
        }
        Err(self.stopped())
    }

    /// Accept callers one after the other, as `UnixListener::incoming` does.
    pub fn incoming(&self) -> impl Iterator<Item = io::Result<PipeStream>> + '_ {
        std::iter::repeat_with(|| self.accept().map(|(stream, ())| stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead as _, BufReader};
    use std::thread;

    fn unique(tag: &str) -> PathBuf {
        use std::sync::atomic::AtomicUsize;
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        PathBuf::from(format!(
            "{PIPE_PREFIX}fxsound-test-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn the_users_pipe_is_named_for_their_sid_and_a_directory_for_its_own() {
        let path = default_path().expect("a SID");
        let text = path.to_str().expect("ASCII");
        assert!(text.starts_with(r"\\.\pipe\fxsound-S-1-"), "{text}");
        let a = path_in(Path::new(r"C:\one")).expect("a SID");
        let b = path_in(Path::new(r"C:\two")).expect("a SID");
        assert_ne!(a, b);
        assert_eq!(a, path_in(Path::new(r"C:\one")).expect("a SID"), "stable");
        assert!(is_pipe_path(&a));
        assert!(!is_pipe_path(Path::new(r"C:\Users\someone\instance.sock")));
        assert!(!is_pipe_path(Path::new(PIPE_PREFIX)));
    }

    #[test]
    fn the_first_instance_is_the_lock_and_a_second_is_refused_until_it_goes() {
        let path = unique("lock");
        let first = PipeListener::bind_first(&path)
            .expect("create")
            .expect("nobody has it");
        assert!(
            PipeListener::bind_first(&path).expect("ask").is_none(),
            "the name is taken while the first instance lives"
        );
        assert!(exists(&path));
        drop(first);
        assert!(
            !exists(&path),
            "the kernel lets the name go with the last handle"
        );
        assert!(PipeListener::bind_first(&path).expect("ask").is_some());
    }

    #[test]
    fn a_line_written_and_closed_is_read_whole_and_then_the_end_of_the_stream() {
        let path = unique("line");
        let listener = PipeListener::bind(&path).expect("bind");
        let server = thread::spawn(move || {
            let (stream, ()) = listener.accept().expect("accept");
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).expect("read");
            (&stream)
                .write_all(format!("echo {line}").as_bytes())
                .expect("write");
            // Dropped at once: what the server does after every answer.
        });
        let client = PipeStream::connect(&path).expect("connect");
        (&client).write_all(b"hello\n").expect("write");
        server.join().expect("server");
        let mut reader = BufReader::new(&client);
        let mut line = String::new();
        reader.read_line(&mut line).expect("read");
        assert_eq!(
            line, "echo hello\n",
            "what was written before the close survives it"
        );
        line.clear();
        assert_eq!(reader.read_line(&mut line).expect("eof"), 0);
    }

    #[test]
    fn a_read_with_nothing_to_read_times_out_and_a_non_blocking_one_would_block() {
        let (ours, theirs) = PipeStream::pair().expect("pair");
        ours.set_read_timeout(Some(Duration::from_millis(30)))
            .expect("timeout");
        let started = Instant::now();
        let e = (&ours)
            .read(&mut [0_u8; 8])
            .expect_err("nothing was written");
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() >= Duration::from_millis(25));

        ours.set_nonblocking(true).expect("non-blocking");
        let e = (&ours).read(&mut [0_u8; 8]).expect_err("still nothing");
        assert_eq!(e.kind(), io::ErrorKind::WouldBlock);
        (&theirs).write_all(b"x").expect("write");
        assert_eq!((&ours).read(&mut [0_u8; 8]).expect("one byte"), 1);
        drop(theirs);
        assert_eq!(
            (&ours).read(&mut [0_u8; 8]).expect("the end"),
            0,
            "a peer that hung up reads as the end of the stream"
        );
        assert!(ours.set_read_timeout(Some(Duration::ZERO)).is_err());
    }

    #[test]
    fn writing_to_a_peer_that_hung_up_is_a_broken_pipe() {
        let (ours, theirs) = PipeStream::pair().expect("pair");
        drop(theirs);
        let e = (&ours).write_all(b"anyone?\n").expect_err("nobody");
        assert_eq!(e.kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn a_stopped_listener_wakes_its_accept_and_accepts_nothing_more() {
        let path = unique("stop");
        let listener = PipeListener::bind(&path).expect("bind");
        let stopper = listener.stopper();
        let waiting = thread::spawn(move || listener.accept().map(|_| ()));
        thread::sleep(Duration::from_millis(50));
        stopper.stop();
        let e = waiting.join().expect("thread").expect_err("stopped");
        assert_eq!(e.kind(), io::ErrorKind::Interrupted);
    }

    #[test]
    fn every_caller_gets_an_instance_and_the_name_never_goes_free_between_them() {
        let path = unique("many");
        let listener = PipeListener::bind(&path).expect("bind");
        let mut clients = Vec::new();
        for _ in 0..5 {
            let client = PipeStream::connect(&path).expect("connect");
            let (server, ()) = listener.accept().expect("accept");
            assert!(
                PipeListener::bind_first(&path).expect("ask").is_none(),
                "a second primary would start here"
            );
            clients.push((client, server));
        }
        assert_eq!(clients.len(), 5);
    }

    #[test]
    fn a_file_is_never_opened_for_a_pipe() {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("instance.sock");
        std::fs::write(&file, b"").expect("write");
        let e = PipeStream::connect(&file).expect_err("not a pipe");
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        assert_eq!(
            std::fs::read(&file).expect("read"),
            b"",
            "nothing written into it"
        );
    }
}
