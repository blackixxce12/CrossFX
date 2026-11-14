//! The Windows backend: WASAPI, the way FxSound for Windows itself plays.
//!
//! ```text
//!  OUTPUT LANE
//!
//!  apps ──► virtual playback device, made the default for the two playback roles (IPolicyConfig)
//!                │  WASAPI loopback capture (AUDCLNT_STREAMFLAGS_LOOPBACK)
//!                ▼
//!           lane DSP: the music chain (crate::lane_dsp, the Linux lanes' own)
//!                │  ring, with the drift between the two clocks taken out (ring.rs)
//!                ▼  WASAPI shared-mode render, converted to the device's format
//!           the user's real speakers
//!
//!  INPUT LANE
//!
//!           the user's real microphone
//!                │  WASAPI shared-mode capture, at 48 kHz
//!                ▼
//!           lane DSP: the voice chain
//!                │  ring
//!                ▼  WASAPI shared-mode render
//!           a virtual cable's playback side ──► its recording side, made the default
//!                                               recording device ──► apps
//! ```
//!
//! This is `audiopassthru/` (`docs/spec/12-audio-io.md` §1–§16) on the port's engine contract:
//! the same [`EngineHandle`], the same [`UiToAudio`] and [`AudioToUi`] with the meanings the
//! Linux engine gives them (`crate::engine`), and the same lane DSP, device rules and paths to the
//! GUI. Only what sits between the chain and the devices is Windows'.
//!
//! # The virtual devices
//!
//! The port ships no driver. It stands on one that is installed: FxSound's own "FxSound Speakers"
//! when the original FxSound is (or was) installed, and otherwise a virtual cable — VB-Audio's or
//! Virtual Audio Cable ([`endpoints`], which says how each is found). The input lane needs a cable
//! of its own besides. With none for a lane the lane is not half-run: it reports
//! [`AudioError::NoVirtualDevice`] (the output lane) or [`AudioError::NoVirtualInputDevice`] (the
//! input lane) and processes nothing. No virtual endpoint is ever offered as a device to pick.
//!
//! # The default devices
//!
//! A lane that is running and wants the default ([`StartOptions::want_default`],
//! [`UiToAudio::SetAsDefault`]) makes its virtual endpoint the default of its direction — the
//! output lane for the two playback roles, leaving calls on the device the user set for them as
//! the original does, the input lane for every role ([`Role::claimed_by`]) — remembering each
//! role's default first, and saying what it was ([`AudioToUi::RememberedDefault`]) so that the
//! settings keep it, and gives each back when it stops: on [`UiToAudio::Shutdown`], on
//! [`UiToAudio::DetachLane`], on `SetAsDefault` with `want: false` (the power off: the sound goes
//! straight to the speakers, and the lane stops), and whenever it ends for good. The playback roles
//! go back to the lane's device, the input lane's communications role to where calls recorded
//! from before ([`defaults`]). The default goes back *before* the streams stop, so the
//! applications are on their way to the real device while the last of what they played is still
//! heard. A move to another real device keeps the default where it is: the applications never
//! notice.
//!
//! The release profile aborts on a panic, and a process can be killed, so no `Drop` can be counted
//! on: a run that dies holding the default leaves it on the virtual device, which swallows every
//! sound. The next start repairs it, as the Linux engine does with the default a killed run left
//! naming its node: once the app has had its moment to attach the lanes ([`REPAIR_PATIENCE`]), a
//! default left on a virtual endpoint that no lane stands behind is given back — to the device the
//! rules remember, the default the settings remembered ([`UiToAudio::SeedRememberedDefaults`])
//! among them, by the `sndDevicesRestoreDefaultDevice` order — and one a lane does stand behind is
//! adopted. "Left" is what FxSound's own record says ([`record`]): every claim is written down
//! before its first write and struck out after its hand-back, so that a cable the user made their
//! default themselves is never taken for one a killed run left. A default on FxSound's own driver
//! needs no record: nothing plays it while neither FxSound runs.
//!
//! The desktop picking another default while a lane holds it is the user picking a device, as the
//! original takes it (`sndDevicesDeviceCallbacks.cpp:95-156`): a lane that follows the system's
//! default device moves there ([`AudioToUi::DesktopPick`]) and takes the default back, as the Linux
//! engine does (`docs/spec/12-audio-io.md` §21, 4a). A role moved that the lane does not follow —
//! calls sent to a headset, or any pick while a ranking decides the lane's device — is left with
//! the user until the claim ends. FxSound's own writes are told apart from the user's by their
//! echoes ([`defaults::Echoes`]), and the original FxSound's, while it runs, are its own.
//!
//! # The original FxSound
//!
//! While FxSound LLC's FxSound runs ([`original`]) it holds the virtual device: the output lane
//! neither takes the default nor processes, a default it held is left to the original rather than
//! handed back into a fight, and the user is told ([`crate::ORIGINAL_FXSOUND_RUNNING`]). The lane
//! picks up once the original has quit. The input lane is not affected: the original has no
//! microphone path and never touches a cable.
//!
//! # Threads
//!
//! One control thread, `fxsound-audio`, in the MTA: the device rules, the defaults, the
//! notifications, the reports — the Linux engine's main loop, woken by the control channel, by the
//! notifications and by its 200 ms supervisor tick. One stream thread per running lane,
//! registered with MMCSS ([`stream`]): the Linux engine's data thread. The control thread starts
//! and stops them, and the lane's DSP travels to the stream thread with its stream and comes back
//! when the stream ends, as it travels to a PipeWire node's user data and back.
//!
//! # What has no Windows meaning
//!
//! Echo cancellation, per-application routes and the microphone held awake are PipeWire's, and
//! their messages are taken and ignored: [`Engine::control`] says why for each.

#[allow(unsafe_code)]
mod com;
mod defaults;
mod endpoints;
mod format;
mod original;
mod probe;
mod record;
mod ring;
mod stream;

#[cfg(test)]
mod live;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TrySendError};
use fxsound_core::DeviceDirection;
use fxsound_core::messages::{AudioToUi, DspEvent, DspParams, InputDspParams, Meters, UiToAudio};
use fxsound_core::{AudioDevice, AudioStatus};
use fxsound_dsp::{ChainSpec, InputEngine};
use triple_buffer::TripleBuffer;

use crate::devices::{self, DeviceInfo, Preference, Selection, SelectionMemory};
use crate::lane_dsp::{self, ChainHandover, LaneDsp};
use crate::per_direction::PerDirection;
use crate::{
    AudioEngine, AudioError, DevicePriority, EVENT_QUEUE_LEN, EngineHandle, MAX_QUANTUM_FRAMES,
    ONE_HEADSET_ON_BOTH_LANES, ORIGINAL_FXSOUND_RUNNING, START_TIMEOUT, StartOptions,
};

pub use probe::probe;

use defaults::{Claim, Defaults, Echoes, Role};
use endpoints::{Endpoint, VirtualDevice};
use stream::{Ended, LeftBehind, Opened, Outcome, StartFailure, StreamPlan, StreamShared, Totals};

/// How often the control thread looks at everything whether or not anything woke it: the Linux
/// engine's `SUPERVISOR_PERIOD`.
const SUPERVISOR_PERIOD: Duration = Duration::from_millis(200);

/// The "don't hammer" backoff of `docs/spec/12-audio-io.md` §22, the Linux engine's own.
const BACKOFF_MS: [u64; 6] = [200, 400, 800, 1600, 3200, 5000];

fn backoff(attempts: u32) -> Duration {
    let step = (attempts as usize).min(BACKOFF_MS.len() - 1);
    Duration::from_millis(BACKOFF_MS[step])
}

/// How often the process list is read for the original FxSound.
const ORIGINAL_EVERY: Duration = Duration::from_secs(2);

/// How long after the start a default left on a virtual device waits to be repaired (module docs,
/// "The default devices"): long enough for the app to have told the engine what it remembered
/// and to have attached the lanes it will attach.
const REPAIR_PATIENCE: Duration = Duration::from_secs(3);

/// How long a lane handing its default back plays on before it falls silent: the applications
/// are moving to the real device meanwhile, and what they played into the virtual device until
/// then is still heard rather than cut off.
const HAND_BACK_TAIL: Duration = Duration::from_millis(60);

/// How long a lane waits for the device it was on after the system wakes: the Linux engine's
/// `RETURN_WAIT`. A Bluetooth headset reconnects a few seconds after the system does.
const RETURN_WAIT: Duration = Duration::from_millis(2500);

/// How long after the wake a lane stays silent at most while it waits: the Linux engine's
/// `WAKE_MUTE`.
const WAKE_MUTE: Duration = Duration::from_secs(2);

/// How long a sleep the app never ended is believed: the Linux engine's `SLEEP_LIMIT`.
const SLEEP_LIMIT: Duration = Duration::from_secs(60);

/// The GUI's end of the control channel.
///
/// `send` has the signature of `pipewire::channel::Sender::send`, which is what
/// [`AudioEngine`] is written against on Linux. The control thread waits on this channel and on
/// its own events at once (`crossbeam_channel::select!`), so a message wakes it with no event of
/// its own to set.
pub(crate) struct Control {
    tx: Sender<UiToAudio>,
}

impl Control {
    /// Hand `message` to the audio thread; the message back when the thread is gone.
    pub(crate) fn send(&self, message: UiToAudio) -> Result<(), UiToAudio> {
        self.tx.send(message).map_err(|error| error.into_inner())
    }
}

/// What reaches the control thread from anywhere but the GUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Event {
    /// An endpoint was added, removed, or changed state.
    Devices,
    /// The default endpoint of a direction for a role changed — to `id`, or to none.
    DefaultChanged {
        direction: DeviceDirection,
        role: Role,
        id: Option<String>,
    },
    /// The virtual device's own volume or mute changed.
    Volume(DeviceDirection),
    /// A lane's stream thread ended, asked to or not.
    StreamEnded {
        direction: DeviceDirection,
        generation: u64,
    },
}

/// Start the WASAPI engine with `options` and hand back the GUI's handle to it.
///
/// Returns once the control thread has joined COM and reached the MMDevice API, which takes a few
/// milliseconds — or failed to, which is [`AudioError::WindowsAudioUnavailable`], the Windows twin
/// of a PipeWire session that is not there.
///
/// # Errors
/// [`AudioError::WindowsAudioUnavailable`] when the thread cannot be started or cannot reach the
/// Windows audio service.
pub(crate) fn start(options: StartOptions) -> Result<EngineHandle, AudioError> {
    let (params_in, params_out) = TripleBuffer::new(&DspParams::default()).split();
    let (input_params_in, input_params_out) = TripleBuffer::new(&InputDspParams::default()).split();
    let (meters_in, meters_out) =
        PerDirection::from_fn(|_| TripleBuffer::new(&Meters::default()).split()).unzip();
    let (events_tx, events_rx) =
        PerDirection::from_fn(|_| crossbeam_channel::bounded(EVENT_QUEUE_LEN)).unzip();
    let (ui_tx, ui_rx) = crossbeam_channel::unbounded();
    let (control_tx, control_rx) = crossbeam_channel::unbounded();
    let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);

    let (dsp, handover) = lane_dsp::build(params_out, input_params_out, meters_in, events_rx);
    // The engine's own way into each chain's event queue: the wake's reset.
    let lane_events = PerDirection {
        output: events_tx.output.clone(),
        input: events_tx.input.clone(),
    };
    let config = Config {
        options,
        control: control_rx,
        notify: ui_tx,
        dsp,
        handover,
        lane_events,
        ready: ready_tx,
    };
    let join = std::thread::Builder::new()
        .name("fxsound-audio".to_owned())
        .spawn(move || run(config))
        .map_err(|error| AudioError::WindowsAudioUnavailable(error.to_string()))?;

    let mut handle = EngineHandle {
        engine: AudioEngine {
            control: Control { tx: control_tx },
            join: Some(join),
        },
        params: params_in,
        input_params: input_params_in,
        meters: meters_out,
        events: events_tx,
        notifications: ui_rx,
    };
    match ready_rx.recv_timeout(START_TIMEOUT) {
        Ok(Ok(())) | Err(crossbeam_channel::RecvTimeoutError::Timeout) => Ok(handle),
        Ok(Err(error)) => {
            handle.engine.stop();
            Err(error)
        }
        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
            handle.engine.stop();
            Err(AudioError::WindowsAudioUnavailable(
                "the audio thread exited before it started".to_owned(),
            ))
        }
    }
}

/// Everything the control thread is started with.
struct Config {
    options: StartOptions,
    control: Receiver<UiToAudio>,
    notify: Sender<AudioToUi>,
    dsp: PerDirection<LaneDsp>,
    handover: ChainHandover,
    lane_events: PerDirection<Sender<DspEvent>>,
    ready: Sender<Result<(), AudioError>>,
}

/// The control thread.
fn run(config: Config) {
    let Config {
        options,
        control,
        notify,
        dsp,
        handover,
        lane_events,
        ready,
    } = config;
    // Declared first, so left last: every COM object of the thread is released before it.
    let _apartment = match com::Apartment::join() {
        Ok(apartment) => apartment,
        Err(error) => {
            let _ = ready.send(Err(AudioError::WindowsAudioUnavailable(error.message())));
            return;
        }
    };
    let (events_tx, events) = crossbeam_channel::unbounded();
    let system = match Windows::new(events_tx.clone()) {
        Ok(system) => system,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    let mut engine = Engine::new(system, notify, dsp, handover, lane_events, &options);
    let started = Instant::now();
    engine.start(started);
    let mut next_tick = started + SUPERVISOR_PERIOD;
    loop {
        let timeout = next_tick.saturating_duration_since(Instant::now());
        let mut shutdown = false;
        crossbeam_channel::select! {
            recv(control) -> message => match message {
                Ok(UiToAudio::Shutdown) | Err(_) => shutdown = true,
                Ok(message) => engine.control(message, Instant::now()),
            },
            recv(events) -> event => {
                if let Ok(event) = event {
                    engine.event(event);
                }
            },
            default(timeout) => {},
        }
        if shutdown {
            break;
        }
        // A burst of endpoint changes is one: everything waiting is taken before acting on it.
        while let Ok(event) = events.try_recv() {
            engine.event(event);
        }
        let now = Instant::now();
        engine.settle(now);
        if now >= next_tick {
            engine.tick(now);
            next_tick = now + SUPERVISOR_PERIOD;
        }
    }
    engine.shutdown();
    drop(engine);
    log::info!("FxSound audio thread stopped");
}

// ---------------------------------------------------------------------------------------------
// What the engine asks of Windows
// ---------------------------------------------------------------------------------------------

/// What the control thread asks of Windows. A trait so that [`Engine`]'s rules are tested
/// against a system the tests describe (`tests`), and never against this machine's defaults.
pub(crate) trait System {
    /// Every active endpoint, both directions.
    ///
    /// # Errors
    /// When the endpoints cannot be listed at all.
    fn endpoints(&mut self) -> Result<Vec<Endpoint>, AudioError>;
    /// Every role's default endpoint of `direction`.
    fn defaults(&mut self, direction: DeviceDirection) -> Defaults;
    /// Make `id` the default endpoint of `direction` for `role`.
    ///
    /// # Errors
    /// When the policy refuses, or there is none to ask.
    fn set_default(
        &mut self,
        direction: DeviceDirection,
        id: &str,
        role: Role,
    ) -> Result<(), AudioError>;
    /// The original FxSound, when it runs, as the log names it.
    fn original_fxsound(&mut self) -> Option<String>;
    /// The endpoint each lane held its direction's defaults on when the last run stopped writing
    /// its record ([`record`]): what a run that was killed leaves behind.
    fn claimed(&mut self) -> PerDirection<Option<String>>;
    /// Keep `claimed` as the record, replacing what it held.
    fn keep_claimed(&mut self, claimed: &PerDirection<Option<String>>);
    /// Start a lane's stream.
    ///
    /// # Errors
    /// Why it could not be started, with what became of the lane's DSP: back in hand, or still on
    /// a thread left to end ([`Ended::left_behind`]).
    fn start_stream(
        &mut self,
        plan: StreamPlan,
        dsp: LaneDsp,
        system_mute: bool,
        gain: f32,
    ) -> Result<Box<dyn LaneStream>, StartFailure>;
    /// Watch the volume of the endpoint `id`, the output lane's virtual device: `None` when it
    /// has none, and the lane then plays at unity.
    fn watch_volume(
        &mut self,
        id: &str,
        direction: DeviceDirection,
    ) -> Option<Box<dyn VolumeWatch>>;
}

/// A lane's running stream, as the control thread holds it.
pub(crate) trait LaneStream {
    fn plan(&self) -> &StreamPlan;
    fn opened(&self) -> Opened;
    fn shared(&self) -> &StreamShared;
    /// Whether its thread has ended without being asked.
    fn has_ended(&self) -> bool;
    /// Fade out after `tail`, end, and hand the DSP back.
    fn stop(self: Box<Self>, tail: Duration) -> Ended;
    /// Collect a stream that ended on its own.
    fn collect(self: Box<Self>) -> Ended;
}

/// A watched endpoint volume.
pub(crate) trait VolumeWatch {
    /// The gain it asks for, as an amplitude; `None` when it cannot be read.
    fn gain(&self) -> Option<f32>;
}

impl LaneStream for stream::Stream {
    fn plan(&self) -> &StreamPlan {
        &self.plan
    }

    fn opened(&self) -> Opened {
        self.opened
    }

    fn shared(&self) -> &StreamShared {
        &self.shared
    }

    fn has_ended(&self) -> bool {
        stream::Stream::has_ended(self)
    }

    fn stop(self: Box<Self>, tail: Duration) -> Ended {
        stream::Stream::stop(*self, tail)
    }

    fn collect(self: Box<Self>) -> Ended {
        stream::Stream::collect(*self)
    }
}

impl VolumeWatch for com::EndpointVolume {
    fn gain(&self) -> Option<f32> {
        com::EndpointVolume::gain(self)
            .map_err(|error| log::debug!("the virtual device's volume: {error}"))
            .ok()
    }
}

/// Windows itself.
struct Windows {
    enumerator: com::Enumerator,
    /// The endpoint notifications, sent to the control thread for as long as this lives.
    _watch: Option<com::Watch>,
    /// `None` when the policy class is not there, which leaves every default where it is.
    policy: Option<com::PolicyConfig>,
    events: Sender<Event>,
    own_pid: u32,
    own_image: Option<String>,
    record: record::Record,
}

impl Windows {
    fn new(events: Sender<Event>) -> Result<Self, AudioError> {
        let (enumerator, watch) = Self::connect(&events)?;
        let policy = com::PolicyConfig::new()
            .map_err(|error| {
                log::error!(
                    "the default-device policy is not there ({error}): FxSound cannot make its \
                     virtual device the default"
                );
            })
            .ok();
        let own_pid = com::own_pid();
        Ok(Self {
            enumerator,
            _watch: watch,
            policy,
            events,
            own_pid,
            own_image: com::process_image(own_pid),
            record: record::Record::of_user(),
        })
    }

    /// A new enumerator, and the endpoint notifications registered with it.
    fn connect(
        events: &Sender<Event>,
    ) -> Result<(com::Enumerator, Option<com::Watch>), AudioError> {
        let enumerator = com::Enumerator::new()
            .map_err(|error| AudioError::WindowsAudioUnavailable(error.message()))?;
        let watch = enumerator
            .watch(events.clone())
            .map_err(|error| log::warn!("no endpoint notifications: {error}"))
            .ok();
        Ok((enumerator, watch))
    }

    /// Every active endpoint, both directions, as the enumerator lists them.
    fn list(&self) -> windows_core::Result<Vec<Endpoint>> {
        let mut all = Vec::new();
        for direction in DeviceDirection::ALL {
            all.extend(self.enumerator.endpoints(direction)?);
        }
        Ok(all)
    }
}

impl System for Windows {
    /// The endpoints — through a new enumerator when the audio service went away under the old
    /// one, as the Linux engine connects again to a PipeWire that restarted
    /// (`docs/spec/12-audio-io.md` §22): the notifications registered with the old one went with
    /// it, and the policy object, which lives in the same service, is made again too. The streams
    /// see the same loss on their own and end with `Outcome::ServiceGone`, which has the
    /// endpoints read again — this — and the lanes retried with the backoff. The old enumerator
    /// is kept until a new one is there, so that a service still on its way back leaves the
    /// engine as it was, to try again on the next read.
    fn endpoints(&mut self) -> Result<Vec<Endpoint>, AudioError> {
        let error = match self.list() {
            Ok(all) => return Ok(all),
            Err(error) => error,
        };
        if !com::is_service_gone(&error) {
            return Err(AudioError::WindowsAudioUnavailable(error.message()));
        }
        log::warn!("the Windows audio service went away ({error}); connecting to it again");
        let (enumerator, watch) = Self::connect(&self.events)?;
        self._watch = watch;
        self.enumerator = enumerator;
        self.policy = com::PolicyConfig::new().ok();
        self.list()
            .map_err(|error| AudioError::WindowsAudioUnavailable(error.message()))
    }

    fn defaults(&mut self, direction: DeviceDirection) -> Defaults {
        self.enumerator.defaults(direction)
    }

    fn set_default(
        &mut self,
        _direction: DeviceDirection,
        id: &str,
        role: Role,
    ) -> Result<(), AudioError> {
        let written = match self.policy.as_ref() {
            Some(policy) => policy.set_default(id, role),
            None => Err(windows::Win32::Foundation::E_NOINTERFACE.into()),
        };
        match written {
            Ok(()) => Ok(()),
            // The audio service restarted under the policy object, or the object was never made:
            // a new one, once, before giving up.
            Err(error) if self.policy.is_none() || com::is_service_gone(&error) => {
                self.policy = com::PolicyConfig::new().ok();
                self.policy
                    .as_ref()
                    .ok_or_else(|| {
                        AudioError::WindowsAudioUnavailable(
                            "the default-device policy is not available".to_owned(),
                        )
                    })?
                    .set_default(id, role)
                    .map_err(|error| AudioError::WindowsAudioUnavailable(error.message()))
            }
            Err(error) => Err(AudioError::WindowsAudioUnavailable(error.message())),
        }
    }

    fn original_fxsound(&mut self) -> Option<String> {
        let processes = com::processes();
        original::find(&processes, self.own_pid, self.own_image.as_deref()).map(|process| {
            format!(
                "{} (process {})",
                process.image.as_deref().unwrap_or(&process.exe),
                process.pid
            )
        })
    }

    fn claimed(&mut self) -> PerDirection<Option<String>> {
        self.record.read()
    }

    fn keep_claimed(&mut self, claimed: &PerDirection<Option<String>>) {
        self.record.write(claimed);
    }

    fn start_stream(
        &mut self,
        plan: StreamPlan,
        dsp: LaneDsp,
        system_mute: bool,
        gain: f32,
    ) -> Result<Box<dyn LaneStream>, StartFailure> {
        stream::Stream::start(plan, dsp, self.events.clone(), system_mute, gain)
            .map(|stream| Box::new(stream) as Box<dyn LaneStream>)
    }

    fn watch_volume(
        &mut self,
        id: &str,
        direction: DeviceDirection,
    ) -> Option<Box<dyn VolumeWatch>> {
        let device = self.enumerator.device(id).ok()?;
        match device.volume(self.events.clone(), direction) {
            Ok(volume) => Some(Box::new(volume)),
            Err(error) => {
                log::info!("{id}: no volume to follow ({error}); the lane plays at unity");
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The engine
// ---------------------------------------------------------------------------------------------

/// How a lane's stream is stopped, and what becomes of its claim on the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// The default stays where it is: the lane moves to another device, or is rebuilt.
    Keep,
    /// The default goes back first: the lane is ending (detached, the power off, shutting down,
    /// no device left).
    HandBack,
    /// The default is left where it is and forgotten: the original FxSound holds it now.
    Abandon,
}

/// One direction's worth of engine state.
struct Lane {
    enabled: bool,
    want_default: bool,
    needs_rules: bool,
    attempts: u32,
    next_attempt: Instant,
    last_error: Option<AudioError>,
    /// The candidates' names at the last run of the rules (`devices::choose_device`, rule 5).
    previous_names: Vec<String>,
    /// The real device the rules chose last.
    target: Option<String>,
    /// The lane is on `target` but plays nothing: the power is off, or the original FxSound runs.
    held: bool,
    /// The virtual device the lane stands on.
    virtual_device: Option<VirtualDevice>,
    /// The DSP, while no stream has it.
    dsp: Option<LaneDsp>,
    stream: Option<Box<dyn LaneStream>>,
    /// A stream's thread that did not end when asked, with the DSP still on it
    /// ([`Ended::left_behind`]): the lane waits for it, and runs again once it has ended.
    left_behind: Option<LeftBehind>,
    /// The virtual device's own volume, for the output lane while it runs.
    volume: Option<Box<dyn VolumeWatch>>,
    /// The lane's claim on its direction's defaults.
    claim: Option<Claim>,
    /// The desktop picked a device the lane follows: the roles it moved are taken back at the
    /// next run of the rules, whether the lane moves or already plays there.
    reclaim: bool,
    /// The device the lane is on was the rules' last resort — nothing remembered, and the default
    /// on a virtual device, as a killed run leaves it — so the default the settings remember
    /// ([`UiToAudio::SeedRememberedDefaults`]) still outranks it when it arrives after the rules
    /// first ran, as it would have had it arrived before.
    blind: bool,
    /// What the GUI was last told the lane is attached to.
    attached: Option<String>,
    last_status: AudioStatus,
    last_cycles: u64,
    /// The format of the lane's last stream, reported while it has none.
    last_format: Option<(u32, u16)>,
    /// Counted by the lane's streams that have ended.
    totals: Totals,
    /// When the lane's current stream was started.
    built_at: Option<Instant>,
    /// After a wake: until when the rules wait for the device the lane was on.
    wake_wait: Option<Instant>,
    /// After a wake: until when the lane stays silent at most.
    wake_mute_until: Option<Instant>,
    /// The engine holds the lane silent (the sleep).
    system_mute: bool,
}

impl Lane {
    fn new(enabled: bool, want_default: bool, dsp: LaneDsp) -> Self {
        Self {
            enabled,
            want_default,
            needs_rules: enabled,
            attempts: 0,
            next_attempt: Instant::now(),
            last_error: None,
            previous_names: Vec::new(),
            target: None,
            held: false,
            virtual_device: None,
            dsp: Some(dsp),
            stream: None,
            left_behind: None,
            volume: None,
            claim: None,
            reclaim: false,
            blind: false,
            attached: None,
            last_status: AudioStatus::default(),
            last_cycles: 0,
            last_format: None,
            totals: Totals::default(),
            built_at: None,
            wake_wait: None,
            wake_mute_until: None,
            system_mute: false,
        }
    }

    fn retry_later(&mut self, now: Instant) {
        self.next_attempt = now + backoff(self.attempts);
        self.attempts = self.attempts.saturating_add(1);
        self.needs_rules = true;
    }

    /// Forget the backoff, and the error it was serving, once the lane's stream has stayed up for
    /// as long as the wait before it (the Linux engine's `Lane::forgive_if_stable`).
    fn forgive_if_stable(&mut self, now: Instant) {
        let Some(built_at) = self.built_at else {
            return;
        };
        if self.stream.is_some() && now >= built_at + backoff(self.attempts.saturating_sub(1)) {
            self.attempts = 0;
            self.last_error = None;
        }
    }

    /// What the lane is attached to, as the GUI is told it: its device while it plays there, or
    /// is held there by the power or by the original FxSound.
    fn attachment(&self) -> Option<&str> {
        if self.enabled && (self.stream.is_some() || self.held) {
            self.target.as_deref()
        } else {
            None
        }
    }
}

/// What one run of a lane's rules chose, and what it spent choosing it.
struct Choice {
    selection: Result<Selection, AudioError>,
    fresh_pick: bool,
    start_over: bool,
}

/// The control thread's state, and every rule of the engine.
pub(crate) struct Engine<S: System> {
    system: S,
    notify: Sender<AudioToUi>,
    /// Every active endpoint, both directions, as last read.
    endpoints: Vec<Endpoint>,
    /// A runtime handle per endpoint ID, for [`AudioDevice::id`]: stable for the run.
    ids: HashMap<String, u32>,
    defaults: PerDirection<Defaults>,
    echoes: PerDirection<Echoes>,
    lanes: PerDirection<Lane>,
    memory: PerDirection<SelectionMemory>,
    preference: PerDirection<Preference>,
    handover: ChainHandover,
    input_spec: ChainSpec,
    input_spec_pending: bool,
    lane_events: PerDirection<Sender<DspEvent>>,
    /// The original FxSound, while it runs.
    original: Option<String>,
    next_original_check: Instant,
    asleep_since: Option<Instant>,
    headset_warned: Option<(String, String)>,
    last_devices: Option<Vec<AudioDevice>>,
    devices_dirty: bool,
    list_dirty: bool,
    /// When the repair of a default left on a virtual device is due — at the start, and once the
    /// original FxSound has quit; `None` once made.
    repair_due: Option<Instant>,
    /// FxSound's record of the endpoint each lane holds its defaults on ([`record`]), as last
    /// kept: at the start, what the last run left.
    claimed: PerDirection<Option<String>>,
    connection_error: Option<AudioError>,
    /// The endpoints could not be read: how many reads have failed in a row, and when the next is
    /// due ([`Self::refresh_endpoints`]).
    endpoint_retry: Option<(u32, Instant)>,
    /// The audio service went away, and the GUI has been told.
    disconnected: bool,
    generation: u64,
}

impl<S: System> Engine<S> {
    fn new(
        mut system: S,
        notify: Sender<AudioToUi>,
        dsp: PerDirection<LaneDsp>,
        handover: ChainHandover,
        lane_events: PerDirection<Sender<DspEvent>>,
        options: &StartOptions,
    ) -> Self {
        let PerDirection { output, input } = dsp;
        if !options.output_enabled {
            log::info!("output lane: starts off");
        }
        if !options.want_default {
            log::info!("both lanes start leaving the default devices where they are");
        }
        // `language` names PipeWire nodes, which Windows has none of, and per-target volumes are
        // FxSound's own nodes' (U10): the volume a Windows lane plays at is its virtual device's.
        let claimed = system.claimed();
        for (direction, endpoint) in claimed.iter() {
            if let Some(endpoint) = endpoint {
                log::info!(
                    "{} lane: the last run recorded holding the default on {endpoint}",
                    direction.key()
                );
            }
        }
        let mut engine = Self {
            system,
            notify,
            endpoints: Vec::new(),
            ids: HashMap::new(),
            defaults: PerDirection::default(),
            echoes: PerDirection::default(),
            lanes: PerDirection {
                output: Lane::new(options.output_enabled, options.want_default, output),
                input: Lane::new(false, options.want_default, input),
            },
            memory: PerDirection::default(),
            preference: PerDirection::default(),
            handover,
            input_spec: ChainSpec::voice(),
            input_spec_pending: false,
            lane_events,
            original: None,
            next_original_check: Instant::now(),
            asleep_since: None,
            headset_warned: None,
            last_devices: None,
            devices_dirty: true,
            list_dirty: true,
            repair_due: None,
            claimed,
            connection_error: None,
            endpoint_retry: None,
            disconnected: false,
            generation: 0,
        };
        for (direction, priority) in [
            (DeviceDirection::Output, &options.output_priority),
            (DeviceDirection::Input, &options.input_priority),
        ] {
            let DevicePriority {
                names,
                new_devices_first,
            } = priority;
            if rank_devices(
                engine.preference.get_mut(direction),
                names.clone(),
                *new_devices_first,
            ) && !engine.preference.get(direction).ranking.is_empty()
            {
                log::info!(
                    "{} lane: starts with {} devices ranked",
                    direction.key(),
                    engine.preference.get(direction).ranking.len()
                );
            }
        }
        engine
    }

    fn notify(&self, message: AudioToUi) {
        if self.notify.send(message).is_err() {
            log::debug!("no GUI is listening for audio notifications");
        }
    }

    /// Report a lane's error once, as the Linux engine does: not again until a stream has proved
    /// itself, whatever is retried meanwhile.
    fn report_error(&mut self, direction: DeviceDirection, error: AudioError) {
        let lane = self.lanes.get_mut(direction);
        if lane
            .last_error
            .as_ref()
            .is_some_and(|last| std::mem::discriminant(last) == std::mem::discriminant(&error))
        {
            log::debug!("audio engine, {} lane, still: {error}", direction.key());
            return;
        }
        log::warn!("audio engine, {} lane: {error}", direction.key());
        lane.last_error = Some(error.clone());
        self.notify(AudioToUi::Error {
            direction: Some(direction),
            message: error.to_string(),
        });
    }

    /// Report an error about the engine as a whole, once.
    fn report_connection_error(&mut self, error: AudioError) {
        if self
            .connection_error
            .as_ref()
            .is_some_and(|last| std::mem::discriminant(last) == std::mem::discriminant(&error))
        {
            return;
        }
        log::warn!("audio engine: {error}");
        self.connection_error = Some(error.clone());
        self.notify(AudioToUi::Error {
            direction: None,
            message: error.to_string(),
        });
    }

    /// The first look at the system: the endpoints, the defaults, the original FxSound; the first
    /// device list; the output lane's first stream, when it is on.
    fn start(&mut self, now: Instant) {
        self.repair_due = Some(now + REPAIR_PATIENCE);
        self.check_original(now);
        self.settle(now);
        self.tick(now);
    }

    // ---- reading the system ------------------------------------------------------------------

    /// Read the endpoints and the defaults again, and tell the GUI what changed. A read that fails
    /// — the audio service gone — is tried again with the backoff ([`Self::tick`]), whether or not
    /// anything else asks for it: with no stream running nothing else would, and the engine would
    /// sit on the list from before the service went.
    fn refresh_endpoints(&mut self, now: Instant) {
        match self.system.endpoints() {
            Ok(endpoints) => {
                self.endpoints = endpoints;
                self.connection_error = None;
                self.endpoint_retry = None;
            }
            Err(error) => {
                let attempts = self.endpoint_retry.map_or(0, |(attempts, _)| attempts);
                self.endpoint_retry = Some((attempts.saturating_add(1), now + backoff(attempts)));
                self.report_connection_error(error);
                return;
            }
        }
        for direction in DeviceDirection::ALL {
            let defaults = self.system.defaults(direction);
            *self.defaults.get_mut(direction) = defaults;
        }
        for endpoint in &self.endpoints {
            let next = u32::try_from(self.ids.len()).unwrap_or(u32::MAX);
            self.ids.entry(endpoint.id.clone()).or_insert(next);
        }
        self.list_dirty = true;
    }

    /// The device list as the GUI wants it: every output by name, then every input by name —
    /// the Linux engine's order, which the GUI draws its sections from.
    fn devices(&self) -> Vec<AudioDevice> {
        let mut published = Vec::new();
        for direction in DeviceDirection::ALL {
            let default = self.defaults.get(direction).main();
            let mut group: Vec<AudioDevice> = self
                .endpoints
                .iter()
                .filter(|endpoint| {
                    endpoint.direction == direction && endpoints::is_offered(endpoint)
                })
                .map(|endpoint| {
                    endpoint
                        .to_audio_device(self.ids.get(&endpoint.id).copied().unwrap_or(0), default)
                })
                .collect();
            group.sort_by(|a, b| a.description.cmp(&b.description));
            published.extend(group);
        }
        published
    }

    /// Every real device, as the device rules see devices.
    fn device_infos(&self) -> Vec<DeviceInfo> {
        self.endpoints
            .iter()
            .filter(|endpoint| endpoints::is_offered(endpoint))
            .map(|endpoint| {
                endpoint.to_device_info(self.ids.get(&endpoint.id).copied().unwrap_or(0))
            })
            .collect()
    }

    /// Whether `id` is an endpoint of a virtual device: listed as one, or the device a lane
    /// stands on.
    fn is_virtual(&self, id: &str) -> bool {
        self.endpoints
            .iter()
            .any(|endpoint| endpoint.id == id && endpoint.virtual_kind().is_some())
            || self.lanes.iter().any(|(direction, lane)| {
                lane.virtual_device
                    .as_ref()
                    .and_then(|device| device.default_endpoint(direction))
                    == Some(id)
            })
    }

    /// Whether `id` is a real device of `direction` that is there now.
    fn is_listed(&self, direction: DeviceDirection, id: &str) -> bool {
        self.endpoints.iter().any(|endpoint| {
            endpoint.id == id && endpoint.direction == direction && endpoints::is_offered(endpoint)
        })
    }

    fn check_original(&mut self, now: Instant) {
        self.next_original_check = now + ORIGINAL_EVERY;
        let running = self.system.original_fxsound();
        if running == self.original {
            return;
        }
        match (&self.original, &running) {
            (None, Some(original)) => {
                log::warn!(
                    "the original FxSound runs ({original}) and holds the virtual device: the \
                     output lane stands aside until it quits"
                );
                self.notify(AudioToUi::Warning {
                    direction: Some(DeviceDirection::Output),
                    message: fxsound_core::i18n::tr(ORIGINAL_FXSOUND_RUNNING),
                });
            }
            (Some(_), None) => {
                log::info!("the original FxSound has quit");
                // It gives its default back as it quits; one it was killed holding is left on its
                // driver, which nothing plays now. Looked at once the output lane has had its
                // moment to take over, as at the start.
                let due = now + REPAIR_PATIENCE;
                self.repair_due = Some(self.repair_due.map_or(due, |pending| pending.min(due)));
            }
            _ => {}
        }
        self.original = running;
        self.lanes.output.needs_rules = self.lanes.output.enabled;
    }

    // ---- the control channel -----------------------------------------------------------------

    /// Act on one control message. Everything but `Shutdown`, which ends the loop in [`run`].
    pub(crate) fn control(&mut self, message: UiToAudio, now: Instant) {
        match message {
            UiToAudio::SelectDevice {
                node_name,
                direction,
            } => self.select_device(direction, node_name, now),
            UiToAudio::DetachLane(direction) => self.detach_lane(direction),
            UiToAudio::RescanDevices => {
                self.devices_dirty = true;
                self.mark_enabled_lanes();
            }
            UiToAudio::SetAsDefault { direction, want } => {
                let lane = self.lanes.get_mut(direction);
                if lane.want_default != want {
                    log::info!(
                        "{} lane: {} the default",
                        direction.key(),
                        if want { "takes" } else { "gives back" }
                    );
                }
                lane.want_default = want;
                lane.needs_rules = lane.enabled;
                if !want {
                    // Now, and on a lane that is off too: whatever it holds goes back.
                    self.stop_lane(direction, Stop::HandBack);
                    let lane = self.lanes.get_mut(direction);
                    lane.held = lane.target.is_some();
                }
            }
            UiToAudio::Restart => {
                for direction in DeviceDirection::ALL {
                    self.stop_lane(direction, Stop::Keep);
                }
                self.devices_dirty = true;
                self.mark_enabled_lanes();
            }
            UiToAudio::Shutdown => self.shutdown(),
            // PipeWire's canceller, loaded into its context (`crate::aec`); Windows has none
            // this engine can load. Answered all the same, as the message promises, so that
            // whoever asked hears it is not running.
            UiToAudio::SetEchoCancel(want) => self.notify(AudioToUi::EchoCancel {
                running: false,
                detail: if want {
                    "echo cancellation is not available on Windows".to_owned()
                } else {
                    String::new()
                },
            }),
            UiToAudio::SeedRememberedDefaults { output, input } => {
                for (direction, name) in [
                    (DeviceDirection::Output, output),
                    (DeviceDirection::Input, input),
                ] {
                    if name.is_empty() || self.is_virtual(&name) {
                        continue;
                    }
                    self.seed_remembered_default(direction, name);
                }
            }
            UiToAudio::SetInputChain(name) => self.set_input_chain(&name),
            UiToAudio::SetDevicePriority {
                direction,
                names,
                new_devices_first,
            } => self.set_device_priority(direction, names, new_devices_first),
            // FxSound's own nodes' volume per real device (U10) is the Linux engine's: a Windows
            // lane plays at its virtual device's own volume, which Windows keeps.
            UiToAudio::SeedTargetVolumes(_) => {}
            UiToAudio::SystemSleeping(true) => self.go_to_sleep(now),
            UiToAudio::SystemSleeping(false) => self.wake_up(now),
            // The Linux input lane records its microphone only while something records from
            // FxSound (Input); this one records it for as long as the lane runs, so there is
            // nothing to hold awake.
            UiToAudio::KeepInputAwake(_) => {}
            // Per-application routes move one application's stream onto a node of its own, which
            // PipeWire lets a client do and Windows does not: an application's endpoint is its
            // own, or the default.
            UiToAudio::SetAppRoutes(_) => {}
        }
        self.apply_idle_events();
    }

    fn mark_enabled_lanes(&mut self) {
        for (_, lane) in self.lanes.iter_mut() {
            lane.needs_rules = lane.enabled;
        }
    }

    /// The settings file's word on what a lane's default was before FxSound last took it.
    ///
    /// It only ever fills a gap, as on Linux: what this run displaced is fresher. With one
    /// exception the Linux engine never meets, because its first pair waits for a round trip to
    /// the server: the rules here run as soon as the endpoints are read, before the app has said
    /// anything, and on a default a killed run left on the virtual device, with nothing remembered,
    /// they can only take the first device there is — and the memory they fill with it is no
    /// knowledge at all ([`Lane::blind`]). The settings' word replaces it, and the rules run again
    /// on it, as though it had come first.
    fn seed_remembered_default(&mut self, direction: DeviceDirection, name: String) {
        let lane = self.lanes.get_mut(direction);
        let memory = self.memory.get_mut(direction);
        if std::mem::take(&mut lane.blind) {
            log::info!(
                "{} lane: the settings remember {name} as the default from before FxSound; the \
                 lane's rules start over from it",
                direction.key()
            );
            memory.original_default.clone_from(&name);
            memory.most_recent_default = name;
            memory.most_recent_playback.clear();
            lane.needs_rules = lane.enabled;
            return;
        }
        if memory.original_default.is_empty() {
            memory.original_default.clone_from(&name);
        }
        if memory.most_recent_default.is_empty() {
            memory.most_recent_default = name;
        }
    }

    /// Record what a run of the rules chose for a lane (`devices::commit`), and whether the choice
    /// was blind ([`Lane::blind`]): made with nothing remembered and no real device the default —
    /// or after such a choice, with nothing learnt since.
    fn commit(&mut self, direction: DeviceDirection, selection: &Selection) {
        let knew = selection.write_previous_default
            || self
                .defaults
                .get(direction)
                .main()
                .is_some_and(|default| !self.is_virtual(default));
        let memory = self.memory.get_mut(direction);
        let empty = *memory == SelectionMemory::default();
        devices::commit(memory, selection);
        let lane = self.lanes.get_mut(direction);
        lane.blind = !knew && (lane.blind || empty);
    }

    /// Attach a lane to the device the user picked, enabling it if it was off (the Linux
    /// engine's `select_device`): the pick is recorded rather than forced onto the system's
    /// default, as `docs/spec/12-audio-io.md` open question 8 asks.
    fn select_device(&mut self, direction: DeviceDirection, node_name: String, now: Instant) {
        let repeated = {
            let lane = self.lanes.get(direction);
            let memory = self.memory.get(direction);
            lane.enabled
                && memory.most_recent_playback == node_name
                && lane.attachment() == Some(node_name.as_str())
        };
        self.memory.get_mut(direction).user_selected = node_name;
        if !repeated {
            self.preference.get_mut(direction).fresh_pick = true;
        }
        let lane = self.lanes.get_mut(direction);
        // The user's word: nothing the rules chose before it was blind any more.
        lane.blind = false;
        if !lane.enabled {
            log::info!("enabling the {} lane", direction.key());
            lane.enabled = true;
            lane.previous_names.clear();
            if direction == DeviceDirection::Output {
                // The microphone's cable may be the speakers' now.
                self.lanes.input.needs_rules = self.lanes.input.enabled;
            }
        }
        let lane = self.lanes.get_mut(direction);
        lane.attempts = 0;
        lane.next_attempt = now;
        lane.last_error = None;
        lane.wake_wait = None;
        self.apply_rules(direction, now);
        self.publish_attachment(direction);
    }

    /// Take a ranking of a lane's devices (the Linux engine's `set_device_priority`).
    fn set_device_priority(
        &mut self,
        direction: DeviceDirection,
        names: Vec<String>,
        new_devices_first: bool,
    ) {
        let memory = self.memory.get(direction);
        let picked =
            !memory.user_selected.is_empty() && memory.user_selected == memory.most_recent_playback;
        // A virtual device is never a device, ranked or not.
        let names: Vec<String> = names
            .into_iter()
            .filter(|name| !self.is_virtual(name))
            .collect();
        let preference = self.preference.get_mut(direction);
        let coming_into_force = preference.ranking.is_empty();
        if !rank_devices(preference, names, new_devices_first) {
            return;
        }
        if preference.ranking.is_empty() {
            log::info!("{} lane: following the system's default", direction.key());
            preference.start_over = false;
        } else if coming_into_force && !picked {
            preference.start_over = true;
        }
        let lane = self.lanes.get_mut(direction);
        lane.needs_rules = lane.enabled;
    }

    /// Hand a lane's default back, stop it and switch it off. Always answered with an
    /// [`AudioToUi::Attached`] carrying `None`, as on Linux.
    fn detach_lane(&mut self, direction: DeviceDirection) {
        log::info!("detaching the {} lane", direction.key());
        self.stop_lane(direction, Stop::HandBack);
        let asleep = self.asleep_since.is_some();
        let lane = self.lanes.get_mut(direction);
        lane.enabled = false;
        lane.needs_rules = false;
        lane.previous_names.clear();
        lane.attempts = 0;
        lane.last_error = None;
        lane.target = None;
        lane.held = false;
        lane.attached = None;
        lane.wake_wait = None;
        lane.wake_mute_until = None;
        if !asleep {
            lane.system_mute = false;
        }
        self.preference.get_mut(direction).fresh_pick = false;
        if direction == DeviceDirection::Output {
            // The speakers' cable may be the microphone's now.
            self.lanes.input.needs_rules = self.lanes.input.enabled;
        }
        self.notify(AudioToUi::Attached {
            direction,
            node_name: None,
        });
    }

    fn set_input_chain(&mut self, name: &str) {
        let spec = ChainSpec::by_name(name).unwrap_or_else(|| {
            log::warn!(
                "the voice preset names a chain this build does not know ({name:?}); running the \
                 voice chain"
            );
            ChainSpec::voice()
        });
        if spec == self.input_spec {
            return;
        }
        log::info!("voice chain: {name}");
        self.input_spec = spec;
        self.input_spec_pending = true;
        self.reconcile_input_chain();
    }

    /// Bring the voice chain up to the spec the preset named, wherever the input lane's DSP is:
    /// rebuilt in place while it is here, or built here and handed over to its stream, which
    /// swaps it in on its next block (`crate::lane_dsp::ChainHandover`).
    fn reconcile_input_chain(&mut self) {
        while self.handover.retired.try_recv().is_ok() {}
        if !self.input_spec_pending {
            return;
        }
        let spec = self.input_spec;
        let lane = &mut self.lanes.input;
        if let Some(dsp) = lane.dsp.as_mut().and_then(LaneDsp::as_input_mut) {
            dsp.set_spec(spec);
            self.input_spec_pending = false;
            return;
        }
        let Some(stream) = lane.stream.as_ref() else {
            return;
        };
        let format = stream.opened().format;
        let engine = Box::new(InputEngine::new_with_spec(
            format.rate as f32,
            MAX_QUANTUM_FRAMES,
            usize::from(format.channels),
            spec,
        ));
        match self.handover.replacement.try_send(engine) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => self.input_spec_pending = false,
            // The last one has not been taken yet: tried again on the next tick.
            Err(TrySendError::Full(_)) => {}
        }
    }

    /// Events for a lane whose DSP is here, not on a stream: applied here, so that its queue does
    /// not fill while the lane is off.
    fn apply_idle_events(&mut self) {
        for (_, lane) in self.lanes.iter_mut() {
            if let Some(dsp) = lane.dsp.as_mut() {
                dsp.drain_events();
            }
        }
    }

    // ---- sleep -------------------------------------------------------------------------------

    fn go_to_sleep(&mut self, now: Instant) {
        if self.asleep_since.is_none() {
            log::info!("the system is going to sleep: both lanes fall silent until it wakes");
            self.asleep_since = Some(now);
        }
        for (_, lane) in self.lanes.iter_mut() {
            lane.system_mute = true;
            if let Some(stream) = lane.stream.as_ref() {
                stream.shared().set_system_mute(true);
            }
            lane.wake_wait = None;
            lane.wake_mute_until = None;
        }
    }

    /// The system woke: both chains forget the world from before the suspend, and each lane's
    /// streams are built again — Windows invalidates most of them across a suspend — with a wait
    /// for the device it was on, silent until it is attached or [`WAKE_MUTE`] is up.
    fn wake_up(&mut self, now: Instant) {
        if self.asleep_since.take().is_none() {
            log::debug!("told the system woke, with no sleep before it");
            return;
        }
        log::info!("the system woke: both lanes look for their devices again");
        for direction in DeviceDirection::ALL {
            if self
                .lane_events
                .get(direction)
                .try_send(DspEvent::ResetFilterState)
                .is_err()
            {
                log::warn!(
                    "could not clear the {} lane's filter history after the wake",
                    direction.key()
                );
            }
            if self.lanes.get(direction).enabled {
                self.stop_lane(direction, Stop::Keep);
                let lane = self.lanes.get_mut(direction);
                lane.wake_wait = Some(now + RETURN_WAIT);
                lane.wake_mute_until = Some(now + WAKE_MUTE);
                lane.next_attempt = now;
                lane.needs_rules = true;
            } else {
                self.lanes.get_mut(direction).system_mute = false;
            }
        }
        self.devices_dirty = true;
        self.apply_idle_events();
    }

    /// Whether a lane's rules wait for the device it was on, after a wake.
    fn waits_after_wake(&mut self, direction: DeviceDirection, now: Instant) -> bool {
        let lane = self.lanes.get(direction);
        let Some(until) = lane.wake_wait else {
            return false;
        };
        if now >= until {
            self.lanes.get_mut(direction).wake_wait = None;
            return false;
        }
        lane.target
            .as_deref()
            .is_some_and(|target| !self.is_listed(direction, target))
    }

    /// Hear a lane again after the wake once it is attached, or [`WAKE_MUTE`] is up.
    fn settle_wake_mute(&mut self, direction: DeviceDirection, now: Instant) {
        let sleeping = self.asleep_since.is_some();
        let lane = self.lanes.get_mut(direction);
        let Some(until) = lane.wake_mute_until else {
            return;
        };
        let attached = lane.stream.is_some() && !lane.needs_rules;
        if sleeping || !(attached || now >= until) {
            return;
        }
        lane.wake_mute_until = None;
        lane.system_mute = false;
        if let Some(stream) = lane.stream.as_ref() {
            stream.shared().set_system_mute(false);
        }
        log::info!("{} lane: heard again after the wake", direction.key());
    }

    // ---- events ------------------------------------------------------------------------------

    pub(crate) fn event(&mut self, event: Event) {
        match event {
            Event::Devices => self.devices_dirty = true,
            Event::DefaultChanged {
                direction,
                role,
                id,
            } => self.default_changed(direction, role, id),
            Event::Volume(direction) => self.follow_volume(direction),
            Event::StreamEnded {
                direction,
                generation,
            } => {
                let ended = self
                    .lanes
                    .get(direction)
                    .stream
                    .as_ref()
                    .is_some_and(|stream| stream.plan().generation == generation);
                if ended {
                    self.stream_ended(direction, Instant::now());
                } else {
                    // An older stream's: perhaps a thread left behind, ending at last.
                    self.collect_left_behind(direction);
                }
            }
        }
    }

    /// A default changed: FxSound's own write coming back, the original FxSound's, or the user
    /// picking a device (module docs, "The default devices").
    fn default_changed(&mut self, direction: DeviceDirection, role: Role, id: Option<String>) {
        self.defaults.get_mut(direction).set(role, id.clone());
        self.list_dirty = true;
        if self.echoes.get_mut(direction).heard(role, id.as_deref()) {
            return;
        }
        // The original FxSound moves the playback default as it starts and as it quits — its
        // claim and its hand-back — and neither is the user's pick.
        if direction == DeviceDirection::Output && self.original.is_some() {
            return;
        }
        let followed = role.is_pick() && self.follow_desktop_pick(direction, id.as_deref());
        let lane = self.lanes.get_mut(direction);
        let Some(claim) = lane.claim.as_mut() else {
            return;
        };
        if id.as_deref() == Some(claim.endpoint.as_str()) {
            return;
        }
        if followed {
            lane.reclaim = true;
        } else {
            // Calls sent to a headset, or a device picked while a ranking decides the lane's: the
            // user's, for as long as this claim stands.
            log::info!(
                "{} lane: the {role:?} default is the user's now, and stays where they put it",
                direction.key()
            );
            claim.yield_role(role);
        }
    }

    /// The desktop picked `picked` as the default device of `direction`: whether the lane takes it
    /// as the user's own pick and follows it there — it is a real device, the lane is on and
    /// follows the system's default device, with no ranking (`docs/spec/12-audio-io.md` §21, 4a).
    fn follow_desktop_pick(&mut self, direction: DeviceDirection, picked: Option<&str>) -> bool {
        let Some(picked) = picked else {
            return false;
        };
        if self.is_virtual(picked) || !self.is_listed(direction, picked) {
            return false;
        }
        let lane = self.lanes.get(direction);
        if !lane.enabled || !self.preference.get(direction).ranking.is_empty() {
            return false;
        }
        let memory = self.memory.get_mut(direction);
        if memory.user_selected != picked {
            log::info!(
                "{} lane: the system's default is now {picked}, which the lane follows",
                direction.key()
            );
            memory.user_selected = picked.to_owned();
            self.notify(AudioToUi::DesktopPick {
                direction,
                node_name: picked.to_owned(),
            });
        }
        let lane = self.lanes.get_mut(direction);
        lane.blind = false;
        lane.needs_rules = true;
        true
    }

    /// The output lane's virtual device changed its volume: the stream plays at it from its next
    /// block.
    fn follow_volume(&mut self, direction: DeviceDirection) {
        let lane = self.lanes.get(direction);
        if let (Some(volume), Some(stream)) = (lane.volume.as_ref(), lane.stream.as_ref()) {
            stream.shared().set_gain(volume.gain().unwrap_or(1.0));
        }
    }

    /// A lane's stream ended without being asked: take its DSP back and decide what comes next.
    fn stream_ended(&mut self, direction: DeviceDirection, now: Instant) {
        let Some(stream) = self.lanes.get_mut(direction).stream.take() else {
            return;
        };
        self.lanes.get_mut(direction).volume = None;
        let ended = stream.collect();
        let outcome = ended.outcome.clone();
        self.take_back(direction, ended);
        match outcome {
            Outcome::Stopped => {}
            // The device went: read the endpoints again and let the rules choose, now.
            Outcome::DeviceGone => {
                self.devices_dirty = true;
                self.lanes.get_mut(direction).needs_rules = true;
            }
            Outcome::ServiceGone(reason) => {
                if !self.disconnected {
                    self.disconnected = true;
                    self.notify(AudioToUi::Disconnected { reason });
                }
                self.devices_dirty = true;
                self.lanes.get_mut(direction).retry_later(now);
            }
            Outcome::Failed(error) => {
                self.report_error(direction, error);
                self.lanes.get_mut(direction).retry_later(now);
            }
        }
    }

    /// Put back what a stream handed back: the DSP, and what it counted — or, from a thread that
    /// did not end in time, the thread, to take them from once it has.
    fn take_back(&mut self, direction: DeviceDirection, ended: Ended) {
        let lane = self.lanes.get_mut(direction);
        lane.totals = lane.totals.plus(ended.totals);
        match (ended.dsp, ended.left_behind) {
            (Some(dsp), _) => {
                debug_assert_eq!(
                    dsp.direction(),
                    direction,
                    "a lane's DSP stays with its lane"
                );
                lane.dsp = Some(dsp);
            }
            (None, Some(thread)) => {
                log::warn!(
                    "the {} lane waits for its last stream's thread to give its DSP back",
                    direction.key()
                );
                lane.left_behind = Some(thread);
            }
            (None, None) => log::error!(
                "the {} lane's DSP did not come back from its stream; the lane cannot run again",
                direction.key()
            ),
        }
    }

    /// Take a lane's DSP back from a thread left behind ([`Lane::left_behind`]) once it has ended,
    /// and let the lane's rules run again.
    fn collect_left_behind(&mut self, direction: DeviceDirection) {
        let lane = self.lanes.get_mut(direction);
        if !lane.left_behind.as_ref().is_some_and(LeftBehind::has_ended) {
            return;
        }
        let Some(thread) = lane.left_behind.take() else {
            return;
        };
        log::info!(
            "the {} lane's last stream thread has ended; its DSP is back",
            direction.key()
        );
        lane.needs_rules = lane.enabled;
        let ended = thread.collect();
        self.take_back(direction, ended);
    }

    // ---- the loop's two beats ----------------------------------------------------------------

    /// After every wake: act on what changed.
    pub(crate) fn settle(&mut self, now: Instant) {
        if self.devices_dirty {
            self.devices_dirty = false;
            self.refresh_endpoints(now);
            self.mark_enabled_lanes();
        }
        if self.list_dirty {
            self.list_dirty = false;
            let devices = self.devices();
            if self.last_devices.as_ref() != Some(&devices) {
                self.last_devices = Some(devices.clone());
                self.notify(AudioToUi::Devices(devices));
            }
        }
        for direction in DeviceDirection::ALL {
            let lane = self.lanes.get(direction);
            if lane.needs_rules && now >= lane.next_attempt {
                self.apply_rules(direction, now);
            }
        }
    }

    /// Every [`SUPERVISOR_PERIOD`]: everything that is looked at whether or not anything said so.
    pub(crate) fn tick(&mut self, now: Instant) {
        if now >= self.next_original_check {
            self.check_original(now);
        }
        if self.endpoint_retry.is_some_and(|(_, due)| now >= due) {
            self.devices_dirty = true;
        }
        if let Some(since) = self.asleep_since
            && now.saturating_duration_since(since) >= SLEEP_LIMIT
        {
            log::warn!(
                "the system said it was going to sleep {SLEEP_LIMIT:?} ago and never said it woke; \
                 both lanes carry on as if it had"
            );
            self.wake_up(now);
        }
        if self.repair_due.is_some_and(|due| now >= due) {
            self.repair_due = None;
            self.repair_defaults();
        }
        for direction in DeviceDirection::ALL {
            if self
                .lanes
                .get(direction)
                .stream
                .as_ref()
                .is_some_and(|stream| stream.has_ended())
            {
                self.stream_ended(direction, now);
            }
            self.collect_left_behind(direction);
            self.lanes.get_mut(direction).forgive_if_stable(now);
            self.settle_wake_mute(direction, now);
        }
        self.reconcile_input_chain();
        self.apply_idle_events();
        self.settle(now);
        for direction in DeviceDirection::ALL {
            self.publish_lane(direction);
        }
        self.warn_of_one_headset_on_both_lanes();
    }

    // ---- the device rules --------------------------------------------------------------------

    /// Ask the device rules for a lane's device, and remember what they saw.
    fn choose(&mut self, direction: DeviceDirection, ours: &str) -> Choice {
        let devices = self.device_infos();
        let selection = devices::choose_device(
            &devices,
            direction,
            ours,
            self.defaults.get(direction).main(),
            &self.lanes.get(direction).previous_names,
            self.memory.get(direction),
            self.preference.get(direction),
        );
        self.lanes.get_mut(direction).previous_names =
            devices::candidates(&devices, direction, ours)
                .into_iter()
                .map(|device| device.name.clone())
                .collect();
        let preference = self.preference.get_mut(direction);
        Choice {
            selection,
            fresh_pick: std::mem::take(&mut preference.fresh_pick),
            start_over: std::mem::take(&mut preference.start_over),
        }
    }

    /// Give back what a run of the rules spent, for a run that made no stream of its choice.
    fn give_back_the_choice(
        &mut self,
        direction: DeviceDirection,
        selection: &Selection,
        fresh_pick: bool,
        start_over: bool,
    ) {
        if fresh_pick && selection.target == self.memory.get(direction).user_selected {
            self.preference.get_mut(direction).fresh_pick = true;
        }
        self.preference.get_mut(direction).start_over |= start_over;
    }

    /// Choose a lane's virtual device and its real device, and make sure its stream runs between
    /// the two — or that it does not, and why.
    fn apply_rules(&mut self, direction: DeviceDirection, now: Instant) {
        let lane = self.lanes.get_mut(direction);
        lane.needs_rules = false;
        if !lane.enabled {
            return;
        }
        // Asleep, waiting for a device after the wake, or with no list of endpoints to choose
        // among — the audio service gone, and the engine reading them again with the backoff: a
        // list from before it went, or none, would have the lane report a device missing that
        // is not (the service's error is the one the GUI has had).
        if self.asleep_since.is_some()
            || self.endpoint_retry.is_some()
            || self.waits_after_wake(direction, now)
        {
            self.lanes.get_mut(direction).needs_rules = true;
            return;
        }

        let input_on = self
            .lanes
            .input
            .virtual_device
            .as_ref()
            .map(|device| device.render.as_str());
        let plan = endpoints::plan(&self.endpoints, self.lanes.output.enabled, input_on);
        let Some(virtual_device) = plan.get(direction).cloned() else {
            self.stop_lane(direction, Stop::HandBack);
            let lane = self.lanes.get_mut(direction);
            lane.target = None;
            lane.held = false;
            lane.virtual_device = None;
            self.report_error(
                direction,
                match direction {
                    DeviceDirection::Output => AudioError::NoVirtualDevice,
                    DeviceDirection::Input => AudioError::NoVirtualInputDevice,
                },
            );
            return;
        };
        if direction == DeviceDirection::Output && self.input_stands_on(&virtual_device.render) {
            // The microphone's cable is the speakers' now — the speakers' lane switched on, or the
            // device it stood on gone. The input lane leaves it first: it hands its recording
            // default back, plays on into the cable while the recorders move, and stops — all
            // before this lane's loopback captures whatever plays into the cable, and claims it,
            // which would otherwise play the microphone out of the speakers until the input lane's
            // own rules ran.
            self.apply_rules(DeviceDirection::Input, now);
            if self.input_stands_on(&virtual_device.render) {
                // Its rules are waiting for something: it stops all the same, and they run later.
                self.stop_lane(DeviceDirection::Input, Stop::HandBack);
                self.lanes.input.needs_rules = self.lanes.input.enabled;
            }
        }
        // Another device is other endpoints: a name is not an identity. "FxSound Speakers" renamed
        // in the Sound control panel is the same endpoint under another friendly name, and the
        // claim on it — the roles the user yielded with it — stands.
        if self
            .lanes
            .get(direction)
            .virtual_device
            .as_ref()
            .is_some_and(|device| {
                device.render != virtual_device.render || device.capture != virtual_device.capture
            })
        {
            // Another virtual device: what the lane holds on the old one goes back first.
            log::info!(
                "{} lane: now on the virtual device {}",
                direction.key(),
                virtual_device.name
            );
            self.stop_lane(direction, Stop::HandBack);
        } else if let Some(device) = self.lanes.get(direction).virtual_device.as_ref()
            && device.name != virtual_device.name
        {
            log::info!(
                "{} lane: the virtual device {} is called {} now",
                direction.key(),
                device.name,
                virtual_device.name
            );
        }
        self.lanes.get_mut(direction).virtual_device = Some(virtual_device.clone());

        let ours = virtual_device
            .default_endpoint(direction)
            .unwrap_or(&virtual_device.render)
            .to_owned();
        let Choice {
            selection,
            fresh_pick,
            start_over,
        } = self.choose(direction, &ours);
        let selection = match selection {
            Ok(selection) => selection,
            Err(error) => {
                // No device to play to or record from: whatever the lane holds goes back.
                self.stop_lane(direction, Stop::HandBack);
                let lane = self.lanes.get_mut(direction);
                lane.target = None;
                lane.held = false;
                self.report_error(direction, error);
                return;
            }
        };

        let original = direction == DeviceDirection::Output && self.original.is_some();
        let lane = self.lanes.get_mut(direction);
        lane.target = Some(selection.target.clone());
        if original || !lane.want_default {
            // On the device, playing nothing: the original FxSound has the virtual device, which
            // is left to it; or the power is off, and the sound goes straight to the device.
            self.stop_lane(
                direction,
                if original {
                    Stop::Abandon
                } else {
                    Stop::HandBack
                },
            );
            self.lanes.get_mut(direction).held = true;
            self.commit(direction, &selection);
            return;
        }
        self.lanes.get_mut(direction).held = false;

        let plan = match direction {
            DeviceDirection::Output => StreamPlan {
                direction,
                capture: virtual_device.render.clone(),
                render: selection.target.clone(),
                generation: 0,
            },
            DeviceDirection::Input => StreamPlan {
                direction,
                capture: selection.target.clone(),
                render: virtual_device.render.clone(),
                generation: 0,
            },
        };
        let running = self
            .lanes
            .get(direction)
            .stream
            .as_ref()
            .is_some_and(|stream| {
                stream.plan().capture == plan.capture && stream.plan().render == plan.render
            });
        if running {
            // Where it should be. A pick the lane follows is taken back: the desktop picked the
            // device it plays to already. Nothing else is: a role the user moved and the lane
            // does not follow stays theirs (`default_changed`).
            let lane = self.lanes.get(direction);
            if lane.claim.is_none() || lane.reclaim {
                self.claim(direction);
            }
            return;
        }

        // Another device: the old stream fades out, the default stays where it is.
        self.stop_lane(direction, Stop::Keep);
        let lane = self.lanes.get(direction);
        if lane.dsp.is_none() && lane.left_behind.is_some() {
            // The DSP is on a thread still stuck in WASAPI; the rules run again once it is back
            // (`collect_left_behind`), with nothing to report meanwhile.
            self.give_back_the_choice(direction, &selection, fresh_pick, start_over);
            return;
        }
        match self.start_stream(direction, plan, now) {
            Ok(()) => {
                // Claim before committing, as on Linux: the claim notes the default from before
                // FxSound, which the commit only fills when it is still empty.
                self.claim(direction);
                self.commit(direction, &selection);
                self.disconnected = false;
            }
            Err(error) => {
                self.report_error(direction, error);
                self.lanes.get_mut(direction).retry_later(now);
                self.give_back_the_choice(direction, &selection, fresh_pick, start_over);
            }
        }
    }

    /// Whether the input lane plays into, or holds the default of, the cable whose playback side
    /// is `render`.
    fn input_stands_on(&self, render: &str) -> bool {
        let lane = &self.lanes.input;
        lane.virtual_device
            .as_ref()
            .is_some_and(|device| device.render == render)
            && (lane.stream.is_some() || lane.claim.is_some())
    }

    /// Start a lane's stream on `plan`'s devices.
    fn start_stream(
        &mut self,
        direction: DeviceDirection,
        mut plan: StreamPlan,
        now: Instant,
    ) -> Result<(), AudioError> {
        let Some(mut dsp) = self.lanes.get_mut(direction).dsp.take() else {
            return Err(AudioError::WindowsAudioUnavailable(
                "the lane's DSP has not come back from its last stream".to_owned(),
            ));
        };
        // Here, where building it is allowed: the voice chain the preset named while the lane was
        // off.
        if let Some(input) = dsp.as_input_mut()
            && self.input_spec_pending
        {
            input.set_spec(self.input_spec);
            self.input_spec_pending = false;
        }
        // A new device starts from a clean chain: the last device's filter history, reverb tail
        // and leveller gain are not carried onto it (the Linux engine's `build_first`).
        dsp.reset();
        self.generation += 1;
        plan.generation = self.generation;
        let volume = match direction {
            DeviceDirection::Output => self.system.watch_volume(&plan.capture, direction),
            DeviceDirection::Input => None,
        };
        let gain = volume
            .as_ref()
            .and_then(|volume| volume.gain())
            .unwrap_or(1.0);
        let system_mute = self.lanes.get(direction).system_mute;
        let started = self.system.start_stream(plan, dsp, system_mute, gain);
        match started {
            Ok(stream) => {
                let opened = stream.opened();
                log::info!(
                    "{} lane: {} ch @ {} Hz, at {:.0} % of the virtual device's volume",
                    direction.key(),
                    opened.format.channels,
                    opened.format.rate,
                    gain * 100.0
                );
                let lane = self.lanes.get_mut(direction);
                lane.stream = Some(stream);
                lane.volume = volume;
                lane.last_cycles = 0;
                lane.last_format = Some((opened.format.rate, opened.format.channels));
                lane.built_at = Some(now);
                Ok(())
            }
            Err((ended, error)) => {
                self.take_back(direction, *ended);
                Err(error)
            }
        }
    }

    /// Stop a lane's stream, handing its default back first, keeping it, or leaving it to the
    /// original FxSound.
    fn stop_lane(&mut self, direction: DeviceDirection, how: Stop) {
        let playing = self.lanes.get(direction).stream.is_some();
        match how {
            Stop::Keep => {}
            Stop::HandBack => self.release(direction),
            Stop::Abandon => {
                if self.lanes.get_mut(direction).claim.take().is_some() {
                    log::info!(
                        "{} lane: the default is the original FxSound's now",
                        direction.key()
                    );
                    // Its to give back, not a killed run's of this port.
                    self.keep_claimed(direction, None);
                }
            }
        }
        let lane = self.lanes.get_mut(direction);
        lane.volume = None;
        let Some(stream) = lane.stream.take() else {
            return;
        };
        let tail = if how == Stop::HandBack && playing {
            HAND_BACK_TAIL
        } else {
            Duration::ZERO
        };
        let ended = stream.stop(tail);
        self.take_back(direction, ended);
    }

    // ---- the defaults ------------------------------------------------------------------------

    /// Make the lane's virtual endpoint the default of its direction for every role the lane takes
    /// ([`Role::claimed_by`]) that it is not yet the default of, remembering each first.
    fn claim(&mut self, direction: DeviceDirection) {
        let lane = self.lanes.get(direction);
        if !lane.want_default {
            return;
        }
        let Some(endpoint) = lane
            .virtual_device
            .as_ref()
            .and_then(|device| device.default_endpoint(direction))
            .map(str::to_owned)
        else {
            return;
        };
        let current = self.system.defaults(direction);
        *self.defaults.get_mut(direction) = current.clone();
        let lane = self.lanes.get_mut(direction);
        lane.reclaim = false;
        let mut claim = lane
            .claim
            .take()
            .filter(|claim| claim.endpoint == endpoint)
            .unwrap_or_else(|| Claim::new(endpoint.clone()));
        // Written down before the first write, so that a run killed at any point after it is
        // known to have left the default here (`record`).
        self.keep_claimed(direction, Some(&endpoint));
        let roles = defaults::claim_roles(
            &current,
            &endpoint,
            Role::claimed_by(direction),
            &mut claim,
            |id| self.is_virtual(id),
        );
        if !roles.is_empty() {
            // Said out loud, so that it reaches the settings file: this memory is exactly what a
            // kill destroys.
            if let Some(before) = claim
                .before
                .main()
                .filter(|before| self.is_listed(direction, before))
                .map(str::to_owned)
            {
                let memory = self.memory.get_mut(direction);
                if memory.original_default.is_empty() {
                    memory.original_default.clone_from(&before);
                }
                memory.most_recent_default.clone_from(&before);
                self.lanes.get_mut(direction).blind = false;
                self.notify(AudioToUi::RememberedDefault {
                    direction,
                    node_name: before,
                });
            }
            for role in roles {
                self.write_default(direction, &endpoint, role);
            }
            log::info!(
                "{} lane: FxSound's virtual device is the default now",
                direction.key()
            );
        }
        self.lanes.get_mut(direction).claim = Some(claim);
    }

    /// Give a lane's claim back (module docs, "The default devices"). A lane holding nothing
    /// writes nothing.
    fn release(&mut self, direction: DeviceDirection) {
        let Some(claim) = self.lanes.get_mut(direction).claim.take() else {
            return;
        };
        self.lanes.get_mut(direction).reclaim = false;
        self.give_back(direction, &claim);
    }

    /// Write every role `claim` still holds back to a real device ([`defaults::hand_back`]), and
    /// strike the claim from the record once none is left on its endpoint.
    fn give_back(&mut self, direction: DeviceDirection, claim: &Claim) {
        let current = self.system.defaults(direction);
        *self.defaults.get_mut(direction) = current.clone();
        let infos = self.device_infos();
        let device =
            devices::restore_default_candidate(self.memory.get(direction), &infos, direction)
                .or_else(|| {
                    // Nothing remembered is here: the device the lane was on, or failing that the
                    // first real device. A default left on the virtual device swallows every sound,
                    // which is worse than any real device.
                    self.lanes
                        .get(direction)
                        .target
                        .clone()
                        .filter(|target| self.is_listed(direction, target))
                        .or_else(|| {
                            infos
                                .iter()
                                .find(|device| device.direction == direction)
                                .map(|device| device.name.clone())
                        })
                });
        let writes = defaults::hand_back(&current, claim, device.as_deref(), |id| {
            self.is_listed(direction, id)
        });
        for (role, target) in writes {
            self.write_default(direction, &target, role);
        }
        if self.defaults.get(direction).any_is(&claim.endpoint) {
            // Kept in the record: the next start gives it back once there is somewhere to.
            log::warn!(
                "{} lane: nothing to give the default back to; it stays on the virtual device",
                direction.key()
            );
        } else {
            log::info!("{} lane: the default is given back", direction.key());
            if self.claimed.get(direction).as_deref() == Some(claim.endpoint.as_str()) {
                self.keep_claimed(direction, None);
            }
        }
    }

    /// Record that the lane of `direction` holds its defaults on `endpoint`, or on nothing.
    fn keep_claimed(&mut self, direction: DeviceDirection, endpoint: Option<&str>) {
        if self.claimed.get(direction).as_deref() == endpoint {
            return;
        }
        *self.claimed.get_mut(direction) = endpoint.map(str::to_owned);
        self.system.keep_claimed(&self.claimed);
    }

    fn write_default(&mut self, direction: DeviceDirection, id: &str, role: Role) {
        match self.system.set_default(direction, id, role) {
            Ok(()) => {
                self.echoes.get_mut(direction).wrote(role, id);
                self.defaults
                    .get_mut(direction)
                    .set(role, Some(id.to_owned()));
                self.list_dirty = true;
            }
            Err(error) => {
                log::warn!("could not make {id} the {role:?} default: {error}");
                self.report_connection_error(error);
            }
        }
    }

    /// The repair of a default a run that did not hand back left on a virtual device (module
    /// docs, "The default devices"): at the start, and once the original FxSound has quit.
    ///
    /// A lane that runs and wants the default has adopted such a default already, by claiming its
    /// endpoint ([`Self::claim`]), and gives it back when it stops. Every other one is given back
    /// now, wherever the lanes stand.
    fn repair_defaults(&mut self) {
        for direction in DeviceDirection::ALL {
            if self.lanes.get(direction).claim.is_some() {
                continue;
            }
            // The original FxSound's default is the original's.
            if direction == DeviceDirection::Output && self.original.is_some() {
                continue;
            }
            let current = self.system.defaults(direction);
            *self.defaults.get_mut(direction) = current.clone();
            let Some(endpoint) = self.left_behind(direction, &current) else {
                // Nothing there of FxSound's: a record that says otherwise is out of date.
                if self.claimed.get(direction).is_some() {
                    self.keep_claimed(direction, None);
                }
                continue;
            };
            log::info!(
                "{} lane: a run before this one left the default on {endpoint}; giving it back",
                direction.key()
            );
            self.give_back(direction, &Claim::new(endpoint));
        }
    }

    /// The virtual endpoint a run that did not hand back left a default of `direction` on, as
    /// `current` stands: the one FxSound's record names ([`record`]), or the playback endpoint of
    /// FxSound's own driver, which nothing plays while neither FxSound runs. A cable the record
    /// does not name may be the user's own default, and is left alone.
    fn left_behind(&self, direction: DeviceDirection, current: &Defaults) -> Option<String> {
        let recorded = self
            .claimed
            .get(direction)
            .clone()
            .filter(|endpoint| current.any_is(endpoint));
        recorded.or_else(|| {
            self.endpoints
                .iter()
                .find(|endpoint| {
                    direction == DeviceDirection::Output
                        && endpoint.direction == direction
                        && endpoint.virtual_kind() == Some(endpoints::VirtualKind::FxSound)
                        && current.any_is(&endpoint.id)
                })
                .map(|endpoint| endpoint.id.clone())
        })
    }

    /// Shut down: every lane's default back, every stream stopped.
    pub(crate) fn shutdown(&mut self) {
        for direction in DeviceDirection::ALL {
            self.stop_lane(direction, Stop::HandBack);
        }
    }

    // ---- the reports -------------------------------------------------------------------------

    fn publish_attachment(&mut self, direction: DeviceDirection) {
        let lane = self.lanes.get_mut(direction);
        let now = lane.attachment().map(str::to_owned);
        if lane.attached == now {
            return;
        }
        lane.attached.clone_from(&now);
        self.notify(AudioToUi::Attached {
            direction,
            node_name: now,
        });
    }

    /// A lane's [`AudioToUi::Attached`] and [`AudioToUi::Status`], each only when it changed — the
    /// attachment first, as on Linux.
    fn publish_lane(&mut self, direction: DeviceDirection) {
        self.publish_attachment(direction);
        let lane = self.lanes.get_mut(direction);
        let (cycles, live, format) = match lane.stream.as_ref() {
            Some(stream) => {
                let shared = stream.shared();
                let format = stream.opened().format;
                (
                    shared.cycles.load(std::sync::atomic::Ordering::Relaxed),
                    Totals::of(shared),
                    Some((format.rate, format.channels)),
                )
            }
            None => (lane.last_cycles, Totals::default(), None),
        };
        let processing = lane.stream.is_some() && cycles != lane.last_cycles;
        lane.last_cycles = cycles;
        let (rate, channels) = format
            .or(lane.last_format)
            .unwrap_or((crate::DEFAULT_SAMPLE_RATE, 2));
        let totals = lane.totals.plus(live);
        let status = AudioStatus {
            processing,
            sample_rate: rate,
            channels,
            processed_secs: totals.frames / u64::from(rate.max(1)),
            dropped_frames: totals.dropped,
            underrun_frames: totals.underruns,
            resyncs: totals.resyncs,
            format_mismatches: 0,
        };
        if status != lane.last_status {
            lane.last_status = status;
            self.notify(AudioToUi::Status { direction, status });
        }
    }

    /// Warn, once per attachment, when both lanes are on one Bluetooth headset (the Linux engine's
    /// `warn_of_one_headset_on_both_lanes`, with Windows' hands-free profile for PipeWire's).
    fn warn_of_one_headset_on_both_lanes(&mut self) {
        let (Some(output), Some(input)) = (
            self.lanes.output.attachment().map(str::to_owned),
            self.lanes.input.attachment().map(str::to_owned),
        ) else {
            self.headset_warned = None;
            return;
        };
        if self
            .headset_warned
            .as_ref()
            .is_some_and(|(o, i)| *o == output && *i == input)
        {
            return;
        }
        let find = |id: &str| self.endpoints.iter().find(|endpoint| endpoint.id == id);
        let one = match (find(&output), find(&input)) {
            (Some(o), Some(i)) => endpoints::one_headset(o, i),
            _ => false,
        };
        self.headset_warned = None;
        if !one {
            return;
        }
        log::info!(
            "{output} and {input} are one Bluetooth headset: its music drops to call quality \
             while its microphone records"
        );
        self.notify(AudioToUi::Warning {
            direction: None,
            message: fxsound_core::i18n::tr(ONE_HEADSET_ON_BOTH_LANES),
        });
        self.headset_warned = Some((output, input));
    }
}

/// Put a ranking into `preference` (the Linux engine's `rank_devices`). Whether anything changed.
fn rank_devices(preference: &mut Preference, names: Vec<String>, new_devices_first: bool) -> bool {
    let ranking: Vec<String> = names.into_iter().filter(|name| !name.is_empty()).collect();
    if preference.ranking == ranking && preference.new_devices_first == new_devices_first {
        return false;
    }
    preference.ranking = ranking;
    preference.new_devices_first = new_devices_first;
    true
}
