//! The Windows backend: WASAPI, the way FxSound for Windows itself plays.
//!
//! **Skeleton.** This file holds the backend's seams — the control channel, the wiring of the
//! GUI's paths to both lanes' DSP, the audio thread — and a thread that answers the control
//! channel but plays nothing yet. The engine itself (`docs/1.0.0-windows-port.md`) replaces
//! [`run`]:
//!
//! ```text
//!  apps ──► virtual playback device, made the system default (IPolicyConfig)
//!                │  WASAPI loopback capture (AUDCLNT_STREAMFLAGS_LOOPBACK)
//!                ▼
//!           lane DSP (crate::lane_dsp, the same chain as on Linux)
//!                │  WASAPI shared-mode render
//!                ▼
//!           the real speakers
//! ```
//!
//! The control messages are [`UiToAudio`]'s, the reports [`AudioToUi`]'s, with the meanings
//! `fxsound_core::messages` gives them; a message with no Windows meaning (the echo canceller,
//! per-application routes, WirePlumber's volume) is taken and ignored.

use crossbeam_channel::{Receiver, Sender};
use fxsound_core::messages::{AudioToUi, DspParams, InputDspParams, Meters, UiToAudio};
use triple_buffer::TripleBuffer;

use crate::lane_dsp::{self, LaneDsp};
use crate::per_direction::PerDirection;
use crate::{AudioEngine, AudioError, EVENT_QUEUE_LEN, EngineHandle, StartOptions};

/// The GUI's end of the control channel.
///
/// `send` has the signature of `pipewire::channel::Sender::send`, which is what
/// [`AudioEngine`] is written against on Linux.
pub(crate) struct Control {
    tx: Sender<UiToAudio>,
}

impl Control {
    /// Hand `message` to the audio thread; the message back when the thread is gone.
    pub(crate) fn send(&self, message: UiToAudio) -> Result<(), UiToAudio> {
        self.tx.send(message).map_err(|error| error.into_inner())
    }
}

/// Start the WASAPI engine with `options` and hand back the GUI's handle to it.
///
/// # Errors
/// [`AudioError::WindowsAudioUnavailable`] when the audio thread could not be spawned.
pub(crate) fn start(options: StartOptions) -> Result<EngineHandle, AudioError> {
    let (params_in, params_out) = TripleBuffer::new(&DspParams::default()).split();
    let (input_params_in, input_params_out) = TripleBuffer::new(&InputDspParams::default()).split();
    let (meters_in, meters_out) =
        PerDirection::from_fn(|_| TripleBuffer::new(&Meters::default()).split()).unzip();
    let (events_tx, events_rx) =
        PerDirection::from_fn(|_| crossbeam_channel::bounded(EVENT_QUEUE_LEN)).unzip();
    let (ui_tx, ui_rx) = crossbeam_channel::unbounded();
    let (control_tx, control_rx) = crossbeam_channel::unbounded();

    let (dsp, _handover) = lane_dsp::build(params_out, input_params_out, meters_in, events_rx);
    let join = std::thread::Builder::new()
        .name("fxsound-audio".to_owned())
        .spawn(move || run(&options, &control_rx, &ui_tx, dsp))
        .map_err(|error| AudioError::WindowsAudioUnavailable(error.to_string()))?;

    Ok(EngineHandle {
        engine: AudioEngine {
            control: Control { tx: control_tx },
            join: Some(join),
        },
        params: params_in,
        input_params: input_params_in,
        meters: meters_out,
        events: events_tx,
        notifications: ui_rx,
    })
}

/// The audio thread. Skeleton: says that no virtual device is in place, and waits for
/// [`UiToAudio::Shutdown`].
fn run(
    options: &StartOptions,
    control: &Receiver<UiToAudio>,
    notify: &Sender<AudioToUi>,
    mut dsp: PerDirection<LaneDsp>,
) {
    log::info!(
        "WASAPI backend skeleton started (output lane {})",
        if options.output_enabled { "on" } else { "off" }
    );
    let _ = notify.send(AudioToUi::Devices(Vec::new()));
    let _ = notify.send(AudioToUi::Error {
        direction: None,
        message: AudioError::NoVirtualDevice.to_string(),
    });
    while let Ok(message) = control.recv() {
        // A lane with no device still applies the events the GUI sends it.
        for lane in [&mut dsp.output, &mut dsp.input] {
            lane.drain_events();
        }
        if matches!(message, UiToAudio::Shutdown) {
            break;
        }
    }
}
