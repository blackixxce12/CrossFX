//! What `--self-test` asks of the Windows audio service, read and left alone: whether it runs.
//! Which playback endpoints are there, and which of them is virtual, the engine's own
//! classification says (`crossfx_audio::InstalledCables`): CrossFX processes the sound that
//! reaches a virtual one, so without one it has nothing to process
//! (`crossfx_audio::AudioError::NoVirtualDevice`).
//!
//! Nothing here opens a stream, changes a default or a volume, or plays anything: the service
//! is asked for its state, and nothing else.

use windows::Win32::System::Services::{
    CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatus, SC_MANAGER_CONNECT,
    SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_STATUS,
};
use windows::core::w;

/// The Windows audio service, as the service manager reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Service {
    Running,
    /// Installed and not running: the state the service manager gave, as a word.
    NotRunning(&'static str),
    /// No such service: a Windows without audio, as Server Core is.
    Missing,
    /// The service manager could not be asked.
    Unknown(String),
}

/// `Audiosrv`, the service every WASAPI client talks to.
#[must_use]
pub fn service() -> Service {
    // SAFETY: handles opened here and closed here; `status` outlives the query.
    unsafe {
        let manager = match OpenSCManagerW(None, None, SC_MANAGER_CONNECT) {
            Ok(manager) => manager,
            Err(err) => return Service::Unknown(err.message()),
        };
        let answer = match OpenServiceW(manager, w!("Audiosrv"), SERVICE_QUERY_STATUS) {
            Ok(audio) => {
                let mut status = SERVICE_STATUS::default();
                let answer = match QueryServiceStatus(audio, &raw mut status) {
                    Ok(()) if status.dwCurrentState == SERVICE_RUNNING => Service::Running,
                    Ok(()) => Service::NotRunning(match status.dwCurrentState.0 {
                        1 => "stopped",
                        2 => "starting",
                        3 => "stopping",
                        5 => "resuming",
                        6 => "pausing",
                        7 => "paused",
                        _ => "in an unknown state",
                    }),
                    Err(err) => Service::Unknown(err.message()),
                };
                let _ = CloseServiceHandle(audio);
                answer
            }
            // `ERROR_SERVICE_DOES_NOT_EXIST`.
            Err(err) if super::is(&err, windows::Win32::Foundation::WIN32_ERROR(1060)) => {
                Service::Missing
            }
            Err(err) => Service::Unknown(err.message()),
        };
        let _ = CloseServiceHandle(manager);
        answer
    }
}
