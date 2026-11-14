//! What `--self-test` asks of Windows audio, read and left alone: whether the audio service runs,
//! and which playback endpoints are there — FxSound processes the sound that reaches a virtual
//! one, so without one it has nothing to process (`fxsound_audio::AudioError::NoVirtualDevice`).
//!
//! Nothing here opens a stream, changes a default or a volume, or plays anything: the service
//! is asked for its state, and the endpoints for their names.

use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    DEVICE_STATE_ACTIVE, IMMDeviceEnumerator, MMDeviceEnumerator, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize, STGM_READ,
};
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

/// The names of the playback endpoints in use, as the Sound settings list them.
///
/// # Errors
///
/// If the endpoints cannot be listed: COM, or the MMDevice API, refused.
pub fn playback_devices() -> Result<Vec<String>, String> {
    // SAFETY: COM for this call, on this thread; balanced only when it was taken here.
    let initialised = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    // SAFETY: interfaces created and released here; the property values are dropped (cleared)
    // before COM is let go.
    let names = unsafe {
        (|| -> windows::core::Result<Vec<String>> {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let devices = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
            let mut names = Vec::new();
            for index in 0..devices.GetCount()? {
                let device = devices.Item(index)?;
                let name = device
                    .OpenPropertyStore(STGM_READ)
                    .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName))
                    .map(|value| value.to_string());
                match name {
                    Ok(name) if !name.is_empty() => names.push(name),
                    Ok(_) | Err(_) => names.push(format!("playback device {}", index + 1)),
                }
            }
            Ok(names)
        })()
    };
    if initialised {
        // SAFETY: balances the successful `CoInitializeEx` above, on the same thread.
        unsafe { CoUninitialize() };
    }
    names.map_err(|err| err.message())
}

/// Whether a playback endpoint called `name` is a virtual device FxSound can stand in front of
/// the speakers with: the FxSound driver's ("FxSound Speakers"), or VB-CABLE's
/// ("CABLE Input (VB-Audio Virtual Cable)").
///
/// The engine decides which device it uses (`fxsound_audio`); this is the self-test's question of
/// whether there is one at all, asked by name.
#[must_use]
pub fn is_virtual(name: &str) -> bool {
    let name = name.to_lowercase();
    ["fxsound", "cable input", "vb-audio"]
        .iter()
        .any(|known| name.contains(known))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fxsound_driver_and_vb_cable_are_virtual_and_speakers_are_not() {
        assert!(is_virtual("FxSound Speakers (FxSound Audio Enhancer)"));
        assert!(is_virtual("CABLE Input (VB-Audio Virtual Cable)"));
        assert!(!is_virtual("Speakers (Realtek(R) Audio)"));
        assert!(!is_virtual("Headphones (WH-1000XM4 Stereo)"));
    }
}
