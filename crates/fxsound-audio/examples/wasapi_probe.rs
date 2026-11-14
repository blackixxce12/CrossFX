//! Print what FxSound's Windows audio engine sees, without changing anything: every playback and
//! recording endpoint as the engine classifies it, the virtual device each lane would stand on,
//! the formats the streams would run at, the default devices, the default-device policy, the
//! engine's own record of the defaults it holds, and whether the original FxSound runs.
//!
//! ```text
//! cargo run -p fxsound-audio --example wasapi_probe
//! ```
//!
//! Read only (`fxsound_audio::wasapi_probe`): no stream is opened, no default written. Windows
//! only; elsewhere it says so.

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    match fxsound_audio::wasapi_probe() {
        Ok(report) => {
            print!("{report}");
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("wasapi_probe: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("wasapi_probe: WASAPI is Windows' — on Linux, `pw-dump` shows what the engine sees");
}
