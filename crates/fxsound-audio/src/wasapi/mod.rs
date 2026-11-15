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
//! input lane) — or [`AudioError::VirtualInputDeviceInUse`], when the input lane's cable is there
//! but the user's — and processes nothing. No endpoint of FxSound's driver, and neither side of a
//! cable a lane stands on, is ever offered as a device to pick; a cable no lane stands on is offered
//! like any other device, as every endpoint but its driver's is to the original, and a user's own
//! default there is remembered, played to and given back there. The device rules come to such a
//! cable only where the user chose it, though — picked it, had it remembered, or made it the
//! system's default themselves ([`Engine::device_infos`], "Whose default it is"): a cable just
//! installed is never "a device just added", nor the "first device there is" of a lane or of a
//! hand-back, nor the session default Windows gave it, any of which would send the system's sound
//! into it. The microphone never takes a cable the user has in use, and a default the speakers'
//! lane finds on its cable already, put there by the user rather than by a run of FxSound's or by
//! Windows, stays theirs.
//!
//! # Whose default it is
//!
//! Windows moves defaults by itself. A device just installed is given the first chance at every
//! role of its direction — an endpoint that has never been the default is compared first
//! (Microsoft's "Default Audio Endpoint Selection") — so a virtual cable installed for FxSound's
//! microphone can take the speakers' default and the microphone's, and the machine falls silent;
//! and a device that goes has its roles fall back to another, a cable among them. Neither is the
//! user's choice, and on a cable neither is a device anyone hears. A cable's being the default is
//! therefore never taken for the user's choice of it by itself. What says the user chose a cable is
//! that they picked it in FxSound, that the settings remember it — the lane's saved device, or the
//! default from before FxSound ([`Engine::remembers`]) — or that the desktop moved the default onto
//! it while FxSound ran, from a device still there, onto a cable listed for longer than
//! [`PICK_WAIT`] already: not in the moment it arrived.
//!
//! A default the engine found on a cable at its start with none of that to say whose it is, or saw
//! put on one as the cable arrived or as a fallback, is Windows' ([`defaults::Placements`]). The
//! cable is free for FxSound's lanes, as any cable nobody uses is. A move of Windows' is no pick: no
//! lane follows it, nothing is saved ([`AudioToUi::DesktopPick`] is never sent for it), and a lane
//! holding its claim puts the role back where it was — the one it held taken back, a role of the
//! user's returned to their device. A lane that stands on such a cable adopts the roles it finds
//! there, as it adopts the default a killed run left, and gives them back to a real device; neither
//! the settings ([`AudioToUi::RememberedDefault`]) nor a hand-back ever names the cable. And a
//! playback default Windows put on the cable the microphone plays into is moved off it, since
//! whatever plays there is recorded with the voice. A default Windows put on a cable no lane uses,
//! in a direction whose lane holds nothing, is left where Windows put it: FxSound is not in that
//! path.
//!
//! # The default devices
//!
//! A lane that is running — or, the input lane, waiting on its cable for a recorder ("The
//! microphone") — and wants the default ([`StartOptions::want_default`],
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
//! before its first write and struck out after its hand-back, so that a default on a cable FxSound
//! never held — the user's, or Windows' — is never taken for one a killed run left. A default on
//! FxSound's own driver needs no record: nothing plays it while neither FxSound runs.
//!
//! The desktop picking another default while a lane holds it is the user picking a device, as the
//! original takes it (`sndDevicesDeviceCallbacks.cpp:95-156`): a lane that follows the system's
//! default device moves there ([`AudioToUi::DesktopPick`]) and takes the default back, as the Linux
//! engine does (`docs/spec/12-audio-io.md` §21, 4a) — and so with the power off, the lane holding
//! nothing: the pick is where it plays once it is on. A role moved that the lane does not follow —
//! calls sent to a headset, a cable's recording side made the default for the user's recorders, or
//! any pick while a ranking decides the lane's device — is left with the user until the claim ends.
//! FxSound's own writes are told apart from the user's by their echoes ([`defaults::Echoes`]), and
//! the original FxSound's, while it runs, are its own. Every other change is decided against the
//! endpoints as read after it ([`Engine::decide_default`]): a device plugged in and made the
//! default arrives in one burst of notifications, and decided against the list from before it, the
//! user's pick of it would read as a pick of nothing the lane can follow. A change off an endpoint
//! that has gone — unplugged, disabled — is Windows' own fallback and no pick: the device the user
//! saved stays theirs. Nor is a cable just installed taking the default ("Whose default it is").
//!
//! # The original FxSound
//!
//! While FxSound LLC's FxSound runs ([`original`]) it holds the virtual device: the output lane
//! neither takes the default nor processes, a default it held is left to the original rather than
//! handed back into a fight, and the user is told ([`crate::ORIGINAL_FXSOUND_RUNNING`]). The lane
//! picks up once the original has quit. The input lane is not affected: the original has no
//! microphone path and never touches a cable.
//!
//! It is looked for only where it can run, with an endpoint of its driver listed, and without
//! reading the process list — a snapshot of every process and thread, milliseconds of CPU — on a
//! short beat for as long as the engine runs ([`Engine::check_original`]). Its start shows by
//! itself: the capture of its driver it opens is a session on the driver's endpoint, whose
//! sessions are watched ([`Event::Session`]), and its claim a default moved onto the driver by
//! someone else. Once found, it is watched through its process until it quits.
//!
//! # The microphone
//!
//! The input lane's microphone is open only while something records from the lane's cable — an
//! application holding a running capture session on its recording side — or while the app holds
//! it awake ([`UiToAudio::KeepInputAwake`]: the calibration, the meters), and for
//! [`RECORDER_LINGER`] after: the Linux lane's capture is passive in the same way
//! (`docs/spec/12-audio-io.md` §19.8), and the original stops its stream "to allow PC to sleep if
//! no audio is playing" (§10). Meanwhile the lane is attached and holds the recording default on
//! the cable, where a recorder that starts finds it, but opens nothing: no microphone light, no
//! Bluetooth headset kept in its call profile, no chain running on the microphone's noise floor,
//! no render stream into the cable for the audio engine to mix.
//!
//! Who records is heard, not polled for. The cable's recording side has its sessions watched
//! ([`System::watch_recorders`]): every session another process opens there, and every one's
//! starting, stopping and going, is an [`Event::Session`], and the sessions are asked then —
//! a fraction of a millisecond — and every [`RECORDERS_CHECK`] besides, for a change missed or a
//! watch gone with the audio service. A recorder that starts is heard at once, and a recording
//! starts with the moment the microphone takes to open, as on Linux. Where the changes cannot be
//! heard the sessions are asked on every tick, and an answer that cannot be had counts as a
//! recorder: a microphone open for nothing costs less than one missing from a call.
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
//! Echo cancellation and per-application routes are PipeWire's, and their messages are taken and
//! ignored: [`Engine::control`] says why for each.

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

use defaults::{Claim, Defaults, Echoes, Placed, Placements, Role};
use endpoints::{Endpoint, VirtualDevice, VirtualKind};
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

/// How often the original FxSound is looked at while it runs — its process, watched, which costs
/// a wait of no time ([`Engine::check_original`]) — and how often the process list is read for
/// it while the output lane captures FxSound's driver without a watch on the driver's sessions.
const ORIGINAL_EVERY: Duration = Duration::from_secs(2);

/// How often the process list is read for the original FxSound otherwise, while an endpoint of its
/// driver is listed: a fallback. Its start is seen at once without it — the loopback capture of
/// its driver it opens is a session there ([`Event::Session`]), and its claim a default moved to
/// the driver by someone else — and a snapshot of every process and thread costs milliseconds.
const ORIGINAL_FALLBACK: Duration = Duration::from_secs(30);

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

/// How long the input lane keeps its microphone open after the last application recording from
/// its cable, or the app holding it awake, has let go of it (module docs, "The microphone"): a
/// recorder that stops and starts again, as a call program between calls may, finds it running.
const RECORDER_LINGER: Duration = Duration::from_secs(3);

/// How often who records from the input lane's cable is asked while every change of it is heard
/// (module docs, "The microphone"): a change missed, or a watch gone with the audio service while
/// no stream runs to notice, costs no more than this. A few sessions read, a fraction of a
/// millisecond each time.
const RECORDERS_CHECK: Duration = Duration::from_secs(2);

/// Who records from a recording endpoint, as [`System::recorders`] tells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Recording {
    /// Another process records from it now — holds a running capture session there; `None` when
    /// that cannot be told.
    pub(crate) active: Option<bool>,
    /// Every change of it is heard ([`Event::Session`]): nothing need ask again until one is.
    pub(crate) heard: bool,
}

impl Recording {
    /// Nothing could be told.
    pub(crate) const UNKNOWN: Self = Self {
        active: None,
        heard: false,
    };
}

/// Who wants the input lane's microphone open, and since when nobody has (module docs, "The
/// microphone").
#[derive(Debug, Default)]
struct Recorders {
    /// The app holds it awake ([`UiToAudio::KeepInputAwake`]): the calibration, the meters.
    awake: bool,
    /// Another process records from the cable's recording side, as last asked — or it could not
    /// be told, which counts as one.
    recording: bool,
    /// The last answer could be had ([`Recording::active`]).
    known: bool,
    /// Every change of who records is heard ([`Recording::heard`]): not asked on every tick.
    heard: bool,
    /// When the last of the app and the recorders let go.
    since: Option<Instant>,
    /// When the cable is asked again whether or not anything said it changed ([`RECORDERS_CHECK`]).
    next_check: Option<Instant>,
}

impl Recorders {
    /// Whether the microphone is wanted at `now`: asked for, or let go of less than
    /// [`RECORDER_LINGER`] ago.
    fn wanted(&self, now: Instant) -> bool {
        self.awake
            || self.recording
            || self
                .since
                .is_some_and(|since| now.saturating_duration_since(since) < RECORDER_LINGER)
    }

    /// The app's word and the sessions' as they stand at `now`.
    fn set(&mut self, awake: bool, recording: bool, now: Instant) {
        let asked = self.awake || self.recording;
        self.awake = awake;
        self.recording = recording;
        if self.awake || self.recording {
            self.since = None;
        } else if asked {
            self.since = Some(now);
        }
    }
}

/// How long a default that moved to an endpoint the engine cannot list yet waits to be decided on
/// ([`Engine::decide_default`]): a device that has just arrived may not have its property store
/// readable for a moment, and the endpoints are read again every [`SUPERVISOR_PERIOD`] meanwhile.
const PICK_WAIT: Duration = Duration::from_secs(2);

/// A change of a default heard from Windows and not decided on yet: the desktop's pick, Windows'
/// own fallback, or a role moved that the lane does not follow ([`Engine::default_changed`]).
#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingDefault {
    /// Where the role went; `None` for no default at all.
    id: Option<String>,
    /// Where it was before, as Windows last reported it — not as the engine last read it, which a
    /// read made between the endpoint going and the report arriving would already show moved.
    before: Option<String>,
    /// When it was heard.
    since: Instant,
}

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
    /// Another process opened an audio session on the endpoint of this ID, one whose sessions the
    /// engine watches ([`System::watch_sessions`], [`System::watch_recorders`]) — or, on one
    /// whose recorders it follows, one of its sessions there started, stopped or went.
    Session(String),
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
                    engine.event(event, Instant::now());
                }
            },
            default(timeout) => {},
        }
        if shutdown {
            break;
        }
        // A burst of endpoint changes is one: everything waiting is taken before acting on it.
        while let Ok(event) = events.try_recv() {
            engine.event(event, Instant::now());
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
    /// Watch the audio sessions of exactly the endpoints `ids` — each session another process
    /// opens on one an [`Event::Session`] — every watch made anew, the ones of other endpoints
    /// dropped; the IDs watched, which leaves out an endpoint that refused.
    fn watch_sessions(&mut self, ids: &[String]) -> Vec<String>;
    /// Watch who records from the recording endpoint `id`, made anew — every session another
    /// process holds there, and every one opened, started, stopped or gone, an [`Event::Session`]
    /// — dropping the watch of any other; `None` watches none.
    fn watch_recorders(&mut self, id: Option<&str>);
    /// Who records from the recording endpoint `id` now.
    fn recorders(&mut self, id: &str) -> Recording;
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
    /// The original FxSound once found, watched through its process, and as the log names it:
    /// while it runs, it is looked at without the process list.
    original: Option<(com::ProcessWatch, String)>,
    /// The endpoints whose sessions are watched ([`System::watch_sessions`]).
    sessions: HashMap<String, com::SessionWatch>,
    /// The recording endpoint whose recorders are watched ([`System::watch_recorders`]).
    recorders: Option<(String, com::SessionWatch)>,
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
            original: None,
            sessions: HashMap::new(),
            recorders: None,
        })
    }

    /// A watch of who records from `id` ([`com::Device::watch_recorders`]), or why there is none.
    fn recorder_watch(&self, id: &str) -> windows_core::Result<com::SessionWatch> {
        self.enumerator
            .device(id)
            .and_then(|device| device.watch_recorders(self.events.clone(), id))
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
        // The sessions watched were the old service's: watched again on the new one.
        self.sessions.clear();
        self.recorders = None;
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

    /// The original FxSound, when it runs. Once found it is watched through its process, and the
    /// process list — a snapshot of every process and thread, milliseconds of CPU — is read again
    /// only once that process has ended.
    fn original_fxsound(&mut self) -> Option<String> {
        if let Some((watch, name)) = self.original.as_ref() {
            if !watch.has_exited() {
                return Some(name.clone());
            }
            self.original = None;
        }
        let processes = com::processes();
        let process = original::find(&processes, self.own_pid, self.own_image.as_deref())?;
        let name = format!(
            "{} (process {})",
            process.image.as_deref().unwrap_or(&process.exe),
            process.pid
        );
        if let Some(watch) = com::ProcessWatch::open(process.pid) {
            self.original = Some((watch, name.clone()));
        }
        Some(name)
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

    /// Made anew, each one: the engine asks after every read of the endpoints, and a watch made
    /// before the audio service restarted — which the endpoints' own enumerator may live through
    /// — tells of nothing.
    fn watch_sessions(&mut self, ids: &[String]) -> Vec<String> {
        // Dropped here, on the control thread: never from inside a callback.
        self.sessions.clear();
        for id in ids {
            let watched = self
                .enumerator
                .device(id)
                .and_then(|device| device.watch_sessions(self.events.clone(), id));
            match watched {
                Ok(watch) => {
                    self.sessions.insert(id.clone(), watch);
                }
                Err(error) => log::info!("{id}: its sessions cannot be watched ({error})"),
            }
        }
        ids.iter()
            .filter(|id| self.sessions.contains_key(*id))
            .cloned()
            .collect()
    }

    fn watch_recorders(&mut self, id: Option<&str>) {
        // Dropped here, on the control thread: never from inside a callback.
        self.recorders = None;
        let Some(id) = id else {
            return;
        };
        match self.recorder_watch(id) {
            Ok(watch) => self.recorders = Some((id.to_owned(), watch)),
            Err(error) => log::info!("{id}: who records from it cannot be watched ({error})"),
        }
    }

    /// Asked of the watch on `id` — made now, when there is none: one refused before is tried
    /// again on every asking, rather than left to the next read of the endpoints.
    fn recorders(&mut self, id: &str) -> Recording {
        if self
            .recorders
            .as_ref()
            .is_none_or(|(watched, _)| watched != id)
        {
            match self.recorder_watch(id) {
                Ok(watch) => self.recorders = Some((id.to_owned(), watch)),
                Err(error) => {
                    log::debug!("{id}: who records from it cannot be watched ({error})");
                    return Recording::UNKNOWN;
                }
            }
        }
        let Some((_, watch)) = self.recorders.as_mut() else {
            return Recording::UNKNOWN;
        };
        match watch.recorders() {
            Ok(heard) => Recording {
                active: Some(heard.recording),
                heard: heard.followed,
            },
            Err(error) => {
                log::debug!("{id}: who records from it cannot be told ({error})");
                Recording::UNKNOWN
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
    /// The input lane is on `target`, holds its default, and keeps the microphone closed: nobody
    /// records from its cable (module docs, "The microphone").
    dormant: bool,
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
    /// on a virtual device, as a killed run leaves it, or on a cable Windows gave it by itself
    /// (module docs, "Whose default it is") — so the default the settings remember
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
            dormant: false,
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
    /// is held there by the power or by the original FxSound, or waits there for a recorder.
    fn attachment(&self) -> Option<&str> {
        if self.enabled && (self.stream.is_some() || self.held || self.dormant) {
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
    /// The defaults as Windows last reported them (`IMMNotificationClient::OnDefaultDeviceChanged`),
    /// from the first read on: what a role was before a change, for telling a pick from a
    /// fallback ([`PendingDefault::before`]). `None` until the first read.
    reported: Option<PerDirection<Defaults>>,
    /// Changes of a default not decided on yet, each role's last ([`Self::decide_defaults`]).
    pending: PerDirection<Vec<(Role, PendingDefault)>>,
    /// The roles whose default Windows put on a virtual cable by itself (module docs, "Whose
    /// default it is"): found there at the start with nothing to say the user put them there, or
    /// moved there as the cable arrived, or as a fallback.
    placed: PerDirection<Placements>,
    /// Each role's default where the first read found it, for as long as nothing has moved it
    /// since — no change of it decided on ([`Self::decide_default`]), none written by FxSound: a
    /// default on a cable found so, whose endpoint may take a read or two to be listed, is one of
    /// [`Self::placed`].
    found: PerDirection<Defaults>,
    /// When each endpoint listed was first listed in this run — `None` for one listed by the first
    /// read, there before FxSound — for telling Windows giving a cable just installed the default
    /// from the user picking one that was there ([`Self::decide_default`]).
    arrived: HashMap<String, Option<Instant>>,
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
    /// When a session opened on FxSound's driver last had the original looked for
    /// ([`Event::Session`]): at once, or by the first read of the process list after it.
    session_look: Option<Instant>,
    /// A session opened on FxSound's driver within [`ORIGINAL_EVERY`] of [`Self::session_look`]
    /// waits for the next read of the process list, which is its look.
    session_waits: bool,
    /// An endpoint of FxSound's driver was listed at the last read: the one thing the original
    /// cannot run without, and so the condition for looking for it at all.
    driver_seen: bool,
    /// The endpoints whose sessions are watched ([`System::watch_sessions`]): the playback ones
    /// of FxSound's driver, where the original's capture shows.
    sessions_watched: Vec<String>,
    /// The recording side of the input lane's cable, whose recorders were last asked to be
    /// watched ([`System::watch_recorders`]).
    recorders_watched: Option<String>,
    /// Who wants the input lane's microphone open.
    recorders: Recorders,
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
            reported: None,
            pending: PerDirection::default(),
            placed: PerDirection::default(),
            found: PerDirection::default(),
            arrived: HashMap::new(),
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
            session_look: None,
            session_waits: false,
            driver_seen: false,
            sessions_watched: Vec::new(),
            recorders_watched: None,
            recorders: Recorders::default(),
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

    /// The first look at the system: the endpoints, the defaults, the original FxSound — looked
    /// for as soon as the first read lists its driver, before any lane's rules run ([`Self::settle`])
    /// — the first device list; the output lane's first stream, when it is on.
    fn start(&mut self, now: Instant) {
        self.repair_due = Some(now + REPAIR_PATIENCE);
        self.settle(now);
        self.tick(now);
    }

    // ---- reading the system ------------------------------------------------------------------

    /// Read the endpoints and the defaults again, and tell the GUI what changed; whether they
    /// could be read. A read that fails — the audio service gone — is tried again with the backoff
    /// ([`Self::tick`]), whether or not anything else asks for it: with no stream running nothing
    /// else would, and the engine would sit on the list from before the service went.
    fn refresh_endpoints(&mut self, now: Instant) -> bool {
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
                return false;
            }
        }
        for direction in DeviceDirection::ALL {
            let defaults = self.system.defaults(direction);
            *self.defaults.get_mut(direction) = defaults;
        }
        // Windows' reports take over from the first read: what each role was before a change is
        // what the last report said, never a read made after the change (`PendingDefault`).
        let first = self.reported.is_none();
        if first {
            self.reported = Some(self.defaults.clone());
            self.found = self.defaults.clone();
        }
        self.arrived = self
            .endpoints
            .iter()
            .map(|endpoint| {
                let at = self
                    .arrived
                    .get(&endpoint.id)
                    .copied()
                    .unwrap_or((!first).then_some(now));
                (endpoint.id.clone(), at)
            })
            .collect();
        for direction in DeviceDirection::ALL {
            let defaults = self.defaults.get(direction).clone();
            // A default found on a cable at the start has nothing yet to say the user put it there
            // (module docs, "Whose default it is"): Windows' until something does.
            let found: Vec<(Role, String)> = Role::ALL
                .into_iter()
                .filter_map(|role| {
                    let id = self.found.get(direction).get(role)?;
                    (defaults.get(role) == Some(id) && self.is_cable(id))
                        .then(|| (role, id.to_owned()))
                })
                .collect();
            let deciding = self.deciding(direction);
            let placed = self.placed.get_mut(direction);
            placed.keep_current(&defaults, |role| deciding.contains(&role));
            for (role, endpoint) in found {
                if !placed.is(role, &endpoint) {
                    placed.set(
                        role,
                        Some(Placed {
                            endpoint,
                            before: None,
                        }),
                    );
                }
            }
        }
        for endpoint in &self.endpoints {
            let next = u32::try_from(self.ids.len()).unwrap_or(u32::MAX);
            self.ids.entry(endpoint.id.clone()).or_insert(next);
        }
        self.list_dirty = true;
        true
    }

    /// The device list as the GUI wants it: every output by name, then every input by name —
    /// the Linux engine's order, which the GUI draws its sections from.
    fn devices(&self) -> Vec<AudioDevice> {
        let ours = self.fxsound_endpoints();
        let mut published = Vec::new();
        for direction in DeviceDirection::ALL {
            let default = self.defaults.get(direction).main();
            let mut group: Vec<AudioDevice> = self
                .endpoints
                .iter()
                .filter(|endpoint| {
                    endpoint.direction == direction && endpoints::is_offered(endpoint, &ours)
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

    /// Every device the rules may choose for a lane (`devices::choose_device`) and give a default
    /// back to ([`Self::give_back`]): every device offered ([`Self::devices`]) but a cable the user
    /// has not chosen for its direction ([`Self::chosen`]).
    ///
    /// A cable no lane stands on is offered like any other device, and the user who picks it, plays
    /// into it, or records from it has it played to and their default given back there. But the
    /// rules never come to it by themselves: installed, it would be "a device just added" (rule 5)
    /// — or, listed before the speakers, the "first device there is" (rule 7, and the hand-back's
    /// last resort), or the session default Windows gave it as it arrived (rules 2 and 6) — and the
    /// system's sound would go into it, where nobody hears it, with the default handed to it at the
    /// exit; and a cable installed for the microphone's lane would be the speakers' device, which
    /// the microphone never mixes into.
    fn device_infos(&self) -> Vec<DeviceInfo> {
        self.offered_infos(|endpoint| {
            endpoint.virtual_kind().is_none() || self.chosen(endpoint.direction, &endpoint.id)
        })
    }

    /// Every device offered ([`Self::devices`]) that `keep` keeps, as the device rules see devices.
    fn offered_infos(&self, keep: impl Fn(&Endpoint) -> bool) -> Vec<DeviceInfo> {
        let ours = self.fxsound_endpoints();
        self.endpoints
            .iter()
            .filter(|endpoint| endpoints::is_offered(endpoint, &ours) && keep(endpoint))
            .map(|endpoint| {
                endpoint.to_device_info(self.ids.get(&endpoint.id).copied().unwrap_or(0))
            })
            .collect()
    }

    /// Whether the user has chosen `id` as the device of the lane of `direction`, as far as the
    /// engine can tell: has the rules remember it ([`Self::remembers`]), or has made it the
    /// system's default device themselves — not Windows by itself (module docs, "Whose default it
    /// is"), and not a run of FxSound's that FxSound's record says left the default there
    /// ([`record`]).
    fn chosen(&self, direction: DeviceDirection, id: &str) -> bool {
        let defaults = self.defaults.get(direction);
        let default = defaults.main_role().is_some_and(|role| {
            defaults.get(role) == Some(id) && !self.placed_by_windows(direction, role, id)
        }) && self.claimed.get(direction).as_deref() != Some(id);
        default || self.remembers(direction, id)
    }

    /// Whether the rules remember `id` for the lane of `direction` ([`SelectionMemory`]): the
    /// user's pick of it — in FxSound's list, or as the system's default the lane followed — a
    /// device the lane was on, or a default from before FxSound, the settings' word on it among
    /// them. Each is a choice of the user's, never a default Windows made by itself, which the
    /// engine neither follows, nor plays to, nor remembers: what says a cable is the user's.
    fn remembers(&self, direction: DeviceDirection, id: &str) -> bool {
        let memory = self.memory.get(direction);
        [
            &memory.user_selected,
            &memory.most_recent_playback,
            &memory.most_recent_default,
            &memory.prior_default,
            &memory.original_default,
        ]
        .into_iter()
        .any(|remembered| !remembered.is_empty() && remembered.as_str() == id)
    }

    /// Whether `role`'s default of `direction` is on `id` because Windows put it there by itself
    /// ([`Placements`]; module docs, "Whose default it is"), and the user has not chosen the cable
    /// since — which outweighs it ([`Self::remembers`]).
    fn placed_by_windows(&self, direction: DeviceDirection, role: Role, id: &str) -> bool {
        self.placed.get(direction).is(role, id) && !self.remembers(direction, id)
    }

    /// Whether `id` is a side of a virtual cable: listed, and of a cable's kind — never FxSound's
    /// driver, whose defaults are FxSound's or the original's.
    fn is_cable(&self, id: &str) -> bool {
        self.endpoints.iter().any(|endpoint| {
            endpoint.id == id && endpoint.virtual_kind().is_some_and(VirtualKind::is_cable)
        })
    }

    /// FxSound's own endpoints, never a device (`endpoints::fxsound_endpoints`): its driver's, and
    /// both sides of the cable each lane stands on — the output lane's by its plan whether or not
    /// it is on, because a cable it would capture is never a device to play to, and the one it
    /// stands on now besides; the input lane's while it is on.
    fn fxsound_endpoints(&self) -> Vec<String> {
        let planned = endpoints::plan(&self.endpoints, false, None, |_| false).output;
        let lanes: Vec<&VirtualDevice> = [
            planned.as_ref(),
            self.lanes
                .output
                .virtual_device
                .as_ref()
                .filter(|_| self.lanes.output.enabled),
            self.lanes
                .input
                .virtual_device
                .as_ref()
                .filter(|_| self.lanes.input.enabled),
        ]
        .into_iter()
        .flatten()
        .collect();
        endpoints::fxsound_endpoints(&self.endpoints, &lanes)
    }

    /// Whether `id` is FxSound's, never the user's device nor the default they had before
    /// FxSound: one of FxSound's own endpoints, the endpoint a lane holds its defaults on — listed
    /// or not — or the one FxSound's record says a run left them on ([`record`]).
    fn is_virtual(&self, id: &str) -> bool {
        self.fxsound_endpoints().iter().any(|ours| ours == id)
            || self.lanes.iter().any(|(direction, lane)| {
                lane.virtual_device
                    .as_ref()
                    .and_then(|device| device.default_endpoint(direction))
                    == Some(id)
            })
            || self
                .claimed
                .iter()
                .any(|(_, endpoint)| endpoint.as_deref() == Some(id))
    }

    /// Whether `id` is an endpoint of FxSound's own driver, which is never a device, whatever the
    /// lanes stand on.
    fn is_driver(&self, id: &str) -> bool {
        self.endpoints.iter().any(|endpoint| {
            endpoint.id == id && endpoint.virtual_kind() == Some(endpoints::VirtualKind::FxSound)
        })
    }

    /// Whether `id` is in the user's own use, so that the input lane takes no cable either side of
    /// which it is (`endpoints::plan`): the real device the output lane plays to — a cable it
    /// plays into — or a default the user has there, which no claim of a lane's, no record of
    /// FxSound's and not Windows by itself put there (module docs, "Whose default it is"). A cable
    /// Windows made the default as it was installed is the microphone's to take.
    fn in_users_use(&self, id: &str) -> bool {
        let played_to =
            self.lanes.output.enabled && self.lanes.output.target.as_deref() == Some(id);
        played_to
            || DeviceDirection::ALL.into_iter().any(|direction| {
                Role::ALL.into_iter().any(|role| {
                    self.defaults.get(direction).get(role) == Some(id)
                        && !self.placed_by_windows(direction, role, id)
                }) && self
                    .lanes
                    .get(direction)
                    .claim
                    .as_ref()
                    .is_none_or(|claim| claim.endpoint != id)
                    && self.claimed.get(direction).as_deref() != Some(id)
            })
    }

    /// Whether `id` is a real device of `direction` that is there now.
    fn is_listed(&self, direction: DeviceDirection, id: &str) -> bool {
        let ours = self.fxsound_endpoints();
        self.endpoints.iter().any(|endpoint| {
            endpoint.id == id
                && endpoint.direction == direction
                && endpoints::is_offered(endpoint, &ours)
        })
    }

    /// Whether the original FxSound runs, and what follows from its starting or quitting.
    ///
    /// Only while an endpoint of its driver is listed: the original shows an error and quits
    /// without its driver (`docs/spec/12-audio-io.md` §15), so a machine with only a cable never
    /// has its processes listed for it, and an original taken to run is taken to have gone with
    /// its driver. Looked at again in [`ORIGINAL_EVERY`] while it runs — its process watched, at
    /// no cost — or while the output lane captures the driver with no watch on its sessions, the
    /// way the original's own capture of it shows; otherwise in [`ORIGINAL_FALLBACK`], its start
    /// being seen at once ([`Event::Session`], a default moved onto its driver).
    fn check_original(&mut self, now: Instant) {
        let running = if self.driver_seen {
            // Whatever has the list read — the beat a session waits for, the fallback's, a default
            // moved onto the driver — answers a session that waits: it is that session's look, and
            // the next one opened waits a beat from it (`Event::Session`).
            if std::mem::take(&mut self.session_waits) {
                self.session_look = Some(now);
            }
            self.system.original_fxsound()
        } else {
            None
        };
        let unwatched = self.lanes.output.stream.as_ref().is_some_and(|stream| {
            self.is_driver(&stream.plan().capture)
                && !self.sessions_watched.contains(&stream.plan().capture)
        });
        self.next_original_check = now
            + if running.is_some() || unwatched {
                ORIGINAL_EVERY
            } else {
                ORIGINAL_FALLBACK
            };
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
                    lane.dormant = false;
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
                    if self.is_cable(&name) {
                        // A cable the settings remember is the user's, whatever its default was
                        // taken for at the start (module docs, "Whose default it is"): what the
                        // lanes may do with it has changed.
                        self.mark_enabled_lanes();
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
            // The input lane's microphone is open only while something records from its cable, as
            // the Linux lane's is passive; the calibration and the meters hold it open meanwhile
            // (module docs, "The microphone").
            UiToAudio::KeepInputAwake(awake) => {
                if awake != self.recorders.awake {
                    log::info!(
                        "input lane: the microphone {} held open",
                        if awake { "is" } else { "is no longer" }
                    );
                }
                let recording = self.recorders.recording;
                self.recorders.set(awake, recording, now);
                self.follow_recorders(now);
            }
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
    /// was blind ([`Lane::blind`]): made with nothing remembered and no device of the user's the
    /// default — a virtual device the default, or a cable Windows made it (module docs, "Whose
    /// default it is") — or after such a choice, with nothing learnt since.
    fn commit(&mut self, direction: DeviceDirection, selection: &Selection) {
        let defaults = self.defaults.get(direction);
        let knew = selection.write_previous_default
            || defaults.main_role().is_some_and(|role| {
                defaults.get(role).is_some_and(|default| {
                    !self.is_virtual(default) && !self.placed_by_windows(direction, role, default)
                })
            });
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
        // FxSound's driver is never a device, ranked or not. A cable is one while no lane stands
        // on it, and the rules only ever see it then, and only once the user has chosen it
        // (`Self::device_infos`) — the app ranks every device it is shown, unasked: ranked, it
        // keeps its place for when it is.
        let names: Vec<String> = names
            .into_iter()
            .filter(|name| !self.is_driver(name))
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
        lane.dormant = false;
        // A lane that is off stands on nothing: the microphone's cable is a device again.
        lane.virtual_device = None;
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
        // What the lists show changed with what the lane stands on.
        self.list_dirty = true;
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

    pub(crate) fn event(&mut self, event: Event, now: Instant) {
        match event {
            Event::Devices => self.devices_dirty = true,
            Event::DefaultChanged {
                direction,
                role,
                id,
            } => self.default_changed(direction, role, id, now),
            Event::Volume(direction) => self.follow_volume(direction),
            Event::Session(endpoint) => {
                if self.original.is_none() && self.is_driver(&endpoint) {
                    // Another process opened a session on FxSound's driver: an application
                    // playing into it, or the original FxSound's capture of it, which is how its
                    // start shows. Looked for now — but not again within `ORIGINAL_EVERY`, however
                    // many sessions are opened: a program that opens one for every sound it plays
                    // would otherwise have the process list, milliseconds of CPU, read for each.
                    // One opened sooner is looked for on that beat, and the look made for it there
                    // is the one the next session waits a beat from (`check_original`): a flood
                    // has the list read once a beat, whatever its pace.
                    match self.session_look {
                        Some(last) if now < last + ORIGINAL_EVERY => {
                            self.session_waits = true;
                            self.next_original_check =
                                self.next_original_check.min(last + ORIGINAL_EVERY);
                        }
                        _ => {
                            self.session_look = Some(now);
                            self.check_original(now);
                        }
                    }
                }
                if self.recorders_watched.as_deref() == Some(endpoint.as_str()) {
                    // A recorder on the input lane's cable opened a session, or started or stopped
                    // recording: the microphone follows now.
                    self.ask_recorders(now);
                    self.follow_recorders(now);
                }
            }
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
                    self.stream_ended(direction, now);
                } else {
                    // An older stream's: perhaps a thread left behind, ending at last.
                    self.collect_left_behind(direction);
                }
            }
        }
    }

    /// A default changed: FxSound's own write coming back, the original FxSound's, Windows' own
    /// fallback, Windows giving a cable just installed the default, or the user picking a device
    /// (module docs, "The default devices", "Whose default it is").
    ///
    /// Only FxSound's echoes and the original's moves are told apart here. Everything else is
    /// decided once the endpoints have been read again ([`Self::decide_defaults`]): the
    /// notifications arrive in bursts, a device plugged in is a dozen state changes and then its
    /// default, and the list the engine last read may not have the device the default went to —
    /// the user's pick of it would be taken for a pick of something the lane cannot follow, and
    /// the role left with the user for as long as the claim stood. Nor can a fallback be told from
    /// a pick but by whether the endpoint the role was on is still there, nor Windows' own default
    /// for a cable but by when the cable arrived.
    fn default_changed(
        &mut self,
        direction: DeviceDirection,
        role: Role,
        id: Option<String>,
        now: Instant,
    ) {
        let before = self
            .reported
            .get_or_insert_with(|| self.defaults.clone())
            .get_mut(direction)
            .replace(role, id.clone());
        self.defaults.get_mut(direction).set(role, id.clone());
        self.list_dirty = true;
        if self.echoes.get_mut(direction).heard(role, id.as_deref()) {
            return;
        }
        if direction == DeviceDirection::Output
            && self.original.is_none()
            && id.as_deref().is_some_and(|id| self.is_driver(id))
        {
            // FxSound's driver made the default by someone else: the original FxSound starting,
            // perhaps, and claiming it. Looked for now, before its claim — and its hand-back as it
            // quits — could be taken for the user's pick.
            self.check_original(now);
        }
        // The original FxSound moves the playback default as it starts and as it quits — its
        // claim and its hand-back — and neither is the user's pick.
        if direction == DeviceDirection::Output && self.original.is_some() {
            return;
        }
        let pending = self.pending.get_mut(direction);
        // A role moved twice before the decision was moved from where it was before the first.
        let before = match pending.iter().position(|(pending, _)| *pending == role) {
            Some(at) => pending.remove(at).1.before,
            None => before,
        };
        pending.push((
            role,
            PendingDefault {
                id,
                before,
                since: now,
            },
        ));
        self.devices_dirty = true;
    }

    /// The roles of `direction` with a change of their default heard and not decided on yet.
    fn deciding(&self, direction: DeviceDirection) -> Vec<Role> {
        self.pending
            .get(direction)
            .iter()
            .map(|(role, _)| *role)
            .collect()
    }

    /// Decide on every change of a default heard since the endpoints were read last — called right
    /// after a read ([`Self::settle`]). One to an endpoint the list does not have yet waits for it
    /// ([`PICK_WAIT`]).
    fn decide_defaults(&mut self, now: Instant) {
        for direction in DeviceDirection::ALL {
            let heard = std::mem::take(self.pending.get_mut(direction));
            for (role, change) in heard {
                if let Some(waiting) = self.decide_default(direction, role, change, now) {
                    self.pending.get_mut(direction).push((role, waiting));
                }
            }
        }
    }

    /// Decide on one change of a default against the endpoints as just read: the change back when
    /// it is to wait for its endpoint to be listed.
    ///
    /// * The role has moved on since, or has no default: nothing to decide — a later report says
    ///   where it went.
    /// * The endpoint the role was on is gone: Windows' fallback (the device unplugged, disabled,
    ///   its driver reinstalled), not a pick. The saved device stays as it is, as for any device
    ///   the lane loses ([`AudioToUi::DesktopPick`] is never sent for it), and the lane's rules run
    ///   on the new default as on any change of the devices. A fallback onto a virtual device that
    ///   nothing plays — the power off, the default left behind on FxSound's driver — has the
    ///   default given back at once ([`Self::repair_defaults`]): nothing would be heard. A fallback
    ///   onto a cable is Windows' placement besides ([`Placements`]).
    /// * The role went to a cable that arrived with the change, or less than [`PICK_WAIT`] before
    ///   it: Windows giving a device just installed the first chance at the default (module docs,
    ///   "Whose default it is"), not a pick either. A lane holding its claim puts the role back
    ///   where it was ([`Self::undo_placement`]); and a role put on the cable the microphone plays
    ///   into is moved off it ([`Self::free_the_microphone_s_cable`]).
    /// * Otherwise it is the desktop's pick: a playback role the lane follows moves the lane and is
    ///   taken back ([`Self::follow_desktop_pick`]); any other role moved off the lane's endpoint is
    ///   the user's for as long as the claim stands ([`Claim::yield_role`]).
    fn decide_default(
        &mut self,
        direction: DeviceDirection,
        role: Role,
        change: PendingDefault,
        now: Instant,
    ) -> Option<PendingDefault> {
        if self.defaults.get(direction).get(role) != change.id.as_deref() {
            return None;
        }
        // A report of the default the role had already moves nothing, and leaves whose it is as it
        // was.
        if change.before == change.id {
            return None;
        }
        self.found.get_mut(direction).set(role, None);
        let Some(id) = change.id.clone() else {
            self.placed.get_mut(direction).set(role, None);
            return None;
        };
        let listed = |id: &str| self.endpoints.iter().any(|endpoint| endpoint.id == id);
        if !listed(&id) {
            if now.saturating_duration_since(change.since) < PICK_WAIT {
                return Some(change);
            }
            log::info!(
                "{} lane: the {role:?} default went to {id}, which never became readable; left \
                 alone",
                direction.key()
            );
            self.placed.get_mut(direction).set(role, None);
            return None;
        }
        let fallback = !change.before.as_deref().is_some_and(listed);
        let arriving = self
            .arrived
            .get(&id)
            .copied()
            .flatten()
            .is_some_and(|arrived| change.since < arrived + PICK_WAIT);
        let placed = self.is_cable(&id) && (fallback || arriving);
        // Windows moving a role off a cable it had put it on itself is one placement: what it was
        // before is where the role was before that, where the user had it.
        let before = match self.placed.get(direction).get(role) {
            Some(earlier)
                if change.before.as_deref() == Some(earlier.endpoint.as_str())
                    && self.placed_by_windows(direction, role, &earlier.endpoint) =>
            {
                earlier.before.clone()
            }
            _ => change.before.clone(),
        };
        self.placed.get_mut(direction).set(
            role,
            placed.then(|| Placed {
                endpoint: id.clone(),
                before: before.clone(),
            }),
        );
        if placed
            && direction == DeviceDirection::Output
            && self.input_render() == Some(id.as_str())
        {
            // Onto the cable the microphone plays into: off it again, once the rules run.
            self.lanes.input.reclaim = true;
        }
        if fallback {
            log::info!(
                "{} lane: Windows moved the {role:?} default to {id}, the device it was on having \
                 gone; no pick of the user's",
                direction.key()
            );
            let current = self.defaults.get(direction).clone();
            if self.lanes.get(direction).claim.is_none()
                && self.left_behind(direction, &current).is_some()
            {
                self.repair_due = Some(now);
            }
            return None;
        }
        if placed {
            log::info!(
                "{} lane: Windows gave the {role:?} default to {id}, a cable just installed; no \
                 pick of the user's",
                direction.key()
            );
            self.undo_placement(direction, role, change.before.as_deref());
            return None;
        }
        let followed = role.is_pick() && self.follow_desktop_pick(direction, Some(&id));
        let lane = self.lanes.get_mut(direction);
        let claim = lane.claim.as_mut()?;
        if id == claim.endpoint {
            return None;
        }
        if followed {
            claim.take_back(role);
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
        None
    }

    /// Put a role Windows has just put on a cable by itself ([`Self::decide_default`]) back where
    /// it was, `before`, when the lane of `direction` holds its claim: FxSound is in that
    /// direction's path, and its defaults are FxSound's to keep right — a role the claim held is
    /// taken back, one of the user's goes back where they had it. A lane holding nothing leaves the
    /// role to Windows, as does a role with nowhere known to go: one found there, or from a device
    /// that is no more.
    fn undo_placement(&mut self, direction: DeviceDirection, role: Role, before: Option<&str>) {
        let Some(claim) = self.lanes.get(direction).claim.as_ref() else {
            return;
        };
        let Some(before) = before else {
            return;
        };
        if before == claim.endpoint
            || (self.is_listed(direction, before) && !self.is_virtual(before))
        {
            let before = before.to_owned();
            log::info!(
                "{} lane: the {role:?} default goes back to {before}",
                direction.key()
            );
            self.write_default(direction, &before, role);
        }
    }

    /// The playback side of the cable the input lane stands on: what it plays the microphone into.
    fn input_render(&self) -> Option<&str> {
        let lane = &self.lanes.input;
        lane.virtual_device
            .as_ref()
            .filter(|_| lane.enabled)
            .map(|device| device.render.as_str())
    }

    /// Move off the playback side of the input lane's cable every playback default Windows put
    /// there by itself (module docs, "Whose default it is"): whatever plays into the cable is
    /// recorded with the voice — the call heard back by whoever is on it, the desktop sent along.
    /// Each goes back where it was, while that is the speakers' lane's device or one of the
    /// user's, and otherwise where the playback defaults are given back to
    /// ([`Self::give_back_device`]). One the user put there is theirs, and keeps the lane off the
    /// cable in the first place ([`Self::in_users_use`]).
    fn free_the_microphone_s_cable(&mut self) {
        let Some(render) = self.input_render().map(str::to_owned) else {
            return;
        };
        let direction = DeviceDirection::Output;
        let current = self.system.defaults(direction);
        *self.defaults.get_mut(direction) = current.clone();
        let leaving = endpoints::device_of(&render, &self.endpoints);
        let speakers = self
            .lanes
            .output
            .claim
            .as_ref()
            .map(|claim| claim.endpoint.clone());
        for role in Role::ALL {
            if current.get(role) != Some(render.as_str())
                || !self.placed_by_windows(direction, role, &render)
            {
                continue;
            }
            let before = self
                .placed
                .get(direction)
                .get(role)
                .and_then(|placed| placed.before.clone())
                .filter(|before| {
                    !leaving.contains(before)
                        && (speakers.as_deref() == Some(before.as_str())
                            || (self.is_listed(direction, before) && !self.is_virtual(before)))
                });
            let Some(target) = before.or_else(|| self.give_back_device(direction, &leaving)) else {
                log::warn!(
                    "input lane: Windows put the {role:?} playback default on {render}, and \
                     there is nowhere to move it to"
                );
                continue;
            };
            log::info!(
                "input lane: Windows put the {role:?} playback default on {render}, which the \
                 microphone plays into; it goes to {target}"
            );
            self.write_default(direction, &target, role);
        }
    }

    /// The desktop picked `picked` as the default device of `direction`: whether the lane takes it
    /// as the user's own pick and follows it there — it is a real device, the lane is on and
    /// follows the system's default device, with no ranking (`docs/spec/12-audio-io.md` §21, 4a).
    ///
    /// The power may be off, and the lane hold no claim: a pick made then is the device the lane
    /// plays to once it is on, as on Linux. Never the recording side of a cable, for the input
    /// lane: that records what plays into the cable's other side, and taken for the microphone it
    /// would send that on into the lane's own cable — a user who records from a cable makes it
    /// their default for their recorders, which is theirs, and left to them ([`Claim::yield_role`]).
    /// Nor is a move Windows made by itself ever asked about here ([`Self::decide_default`]).
    fn follow_desktop_pick(&mut self, direction: DeviceDirection, picked: Option<&str>) -> bool {
        let Some(picked) = picked else {
            return false;
        };
        if self.is_virtual(picked) || !self.is_listed(direction, picked) {
            return false;
        }
        if direction == DeviceDirection::Input && self.is_cable(picked) {
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
            if self.refresh_endpoints(now) {
                self.watch_sessions();
                self.watch_recorders(true);
                let driver = self.endpoints.iter().any(|endpoint| {
                    endpoint.virtual_kind() == Some(endpoints::VirtualKind::FxSound)
                });
                if driver != self.driver_seen {
                    // FxSound's driver listed — at the start too — or gone: the original is looked
                    // for, or let go of, now, before any lane's rules run on it.
                    self.driver_seen = driver;
                    self.check_original(now);
                }
                // Against the endpoints as they are now, never as they were before a burst.
                self.decide_defaults(now);
            }
            self.mark_enabled_lanes();
        }
        self.publish_devices();
        for direction in DeviceDirection::ALL {
            let lane = self.lanes.get(direction);
            if lane.needs_rules && now >= lane.next_attempt {
                self.apply_rules(direction, now);
            }
        }
        // A lane that moved onto a cable, or off one, changed what the lists offer; the input lane
        // detached, or held by the power, has no recorders to watch for.
        self.publish_devices();
        self.watch_recorders(false);
    }

    /// Watch the sessions of the playback endpoints of FxSound's driver: the original FxSound
    /// opens its capture of them as it starts, and that is how its start is seen at once. Made
    /// anew after every read of the endpoints: the audio service may have restarted under the
    /// watches of before.
    fn watch_sessions(&mut self) {
        let ids: Vec<String> = self
            .endpoints
            .iter()
            .filter(|endpoint| {
                endpoint.direction == DeviceDirection::Output
                    && endpoint.virtual_kind() == Some(endpoints::VirtualKind::FxSound)
            })
            .map(|endpoint| endpoint.id.clone())
            .collect();
        self.sessions_watched = self.system.watch_sessions(&ids);
    }

    /// Watch who records from the input lane's cable ([`System::watch_recorders`]; module docs,
    /// "The microphone"): made anew after every read of the endpoints (`renew`), as the sessions
    /// are, and otherwise only when the cable changed, or the lane left it.
    fn watch_recorders(&mut self, renew: bool) {
        if !renew && self.input_capture() == self.recorders_watched.as_deref() {
            return;
        }
        let capture = self.input_capture().map(str::to_owned);
        self.system.watch_recorders(capture.as_deref());
        self.recorders_watched = capture;
    }

    /// The recording side of the cable the input lane stands on, while it is on and the power is:
    /// what a recorder of FxSound's microphone records from.
    fn input_capture(&self) -> Option<&str> {
        let lane = &self.lanes.input;
        lane.virtual_device
            .as_ref()
            .filter(|_| lane.enabled && !lane.held)
            .and_then(|device| device.capture.as_deref())
    }

    /// Ask who records from the input lane's cable (module docs, "The microphone"). An answer that
    /// cannot be had counts as a recorder — a microphone open for nothing costs less than one
    /// missing from a call — and, the first time, has the endpoints read again, and the watch made
    /// anew with them: the audio service may have gone with it, and nothing else would notice
    /// while no stream runs.
    fn ask_recorders(&mut self, now: Instant) {
        let Some(capture) = self.input_capture().map(str::to_owned) else {
            return;
        };
        let heard = self.system.recorders(&capture);
        let recording = heard.active.unwrap_or(true);
        if heard.active.is_none() {
            if self.recorders.known {
                log::info!(
                    "input lane: who records from its cable cannot be told; its microphone stays \
                     open, and the endpoints are read again"
                );
                self.devices_dirty = true;
            }
        } else if recording != self.recorders.recording {
            log::info!(
                "input lane: {} records from its cable",
                if recording { "something" } else { "nothing" }
            );
        }
        self.recorders.known = heard.active.is_some();
        self.recorders.heard = heard.heard;
        self.recorders.next_check = Some(now + RECORDERS_CHECK);
        let awake = self.recorders.awake;
        self.recorders.set(awake, recording, now);
    }

    /// Have the input lane's rules run when its microphone's being open no longer matches whether
    /// it is wanted: a recorder came, or the last went and its moment is up.
    fn follow_recorders(&mut self, now: Instant) {
        let wanted = self.recorders.wanted(now);
        let lane = &mut self.lanes.input;
        if lane.enabled && ((wanted && lane.dormant) || (!wanted && lane.stream.is_some())) {
            lane.needs_rules = true;
        }
    }

    /// The device list, when something it is made of changed and it reads otherwise than the GUI
    /// was last told.
    fn publish_devices(&mut self) {
        if !self.list_dirty {
            return;
        }
        self.list_dirty = false;
        let devices = self.devices();
        if self.last_devices.as_ref() != Some(&devices) {
            self.last_devices = Some(devices.clone());
            self.notify(AudioToUi::Devices(devices));
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
        // A default that went to an endpoint not listed yet: read again until it is, or its wait
        // is up (`decide_default`).
        if self.endpoint_retry.is_none() && self.pending.iter().any(|(_, heard)| !heard.is_empty())
        {
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
        // Who records from the input lane's cable: asked on every tick where its changes are not
        // heard, and otherwise every `RECORDERS_CHECK` — and a microphone kept open for a recorder
        // that has stopped is let go of once its moment is up (module docs, "The microphone").
        if self.input_capture().is_some() {
            if !self.recorders.heard || self.recorders.next_check.is_none_or(|due| now >= due) {
                self.ask_recorders(now);
            }
            self.follow_recorders(now);
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
        // What was there at this run, as the next one tells a device just added (rule 5, a
        // ranking's newcomer): every device offered, a cable the rules leave alone among them. One
        // the user chooses later — makes it the system's default, say, which a ranking leaves with
        // them — has been there all along, and is no newcomer to take the lane.
        let offered = self.offered_infos(|_| true);
        self.lanes.get_mut(direction).previous_names =
            devices::candidates(&offered, direction, ours)
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
        let plan = endpoints::plan(&self.endpoints, self.lanes.output.enabled, input_on, |id| {
            self.in_users_use(id)
        });
        let Some(virtual_device) = plan.get(direction).cloned() else {
            self.stop_lane(direction, Stop::HandBack);
            let lane = self.lanes.get_mut(direction);
            lane.target = None;
            lane.held = false;
            lane.dormant = false;
            if lane.virtual_device.take().is_some() {
                // Its cable is a device again.
                self.list_dirty = true;
            }
            self.report_error(
                direction,
                match direction {
                    DeviceDirection::Output => AudioError::NoVirtualDevice,
                    // There is a cable, the user's: they need to know to free it, not to install one.
                    DeviceDirection::Input if plan.input_in_use => {
                        AudioError::VirtualInputDeviceInUse
                    }
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
        let lane = self.lanes.get_mut(direction);
        if lane.virtual_device.as_ref() != Some(&virtual_device) {
            // What the lists offer changed with what the lane stands on.
            self.list_dirty = true;
        }
        lane.virtual_device = Some(virtual_device.clone());

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
                lane.dormant = false;
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
            let lane = self.lanes.get_mut(direction);
            lane.held = true;
            lane.dormant = false;
            self.commit(direction, &selection);
            return;
        }
        self.lanes.get_mut(direction).held = false;
        if direction == DeviceDirection::Input {
            // Who records from this cable, asked now: whether the microphone opens is decided on
            // it, and an answer from another cable, or from before the change that runs these
            // rules, would be no answer (module docs, "The microphone").
            self.watch_recorders(false);
            self.ask_recorders(now);
        }
        if direction == DeviceDirection::Input && !self.recorders.wanted(now) {
            // Nobody records from the cable and the app does not hold the microphone awake: it
            // stays closed — its light off, a Bluetooth headset left in its music profile, the
            // chain idle — while the recording default stays on the cable, where a recorder that
            // starts finds it, and is heard (`Event::Session`). The lane is where it should be:
            // what it reported before, and the backoff it served, are over.
            self.stop_lane(direction, Stop::Keep);
            let lane = self.lanes.get_mut(direction);
            lane.dormant = true;
            lane.attempts = 0;
            lane.last_error = None;
            if lane.claim.is_none() || lane.reclaim {
                self.claim(direction);
            }
            self.commit(direction, &selection);
            self.disconnected = false;
            return;
        }
        self.lanes.get_mut(direction).dormant = false;

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
        let capture = plan.capture.clone();
        let started = self.system.start_stream(plan, dsp, system_mute, gain);
        match started {
            Ok(stream) => {
                if self.is_driver(&capture) && !self.sessions_watched.contains(&capture) {
                    // Capturing the driver the original would capture, with nothing to show it
                    // starting: the process list is read on the short beat (`check_original`).
                    self.next_original_check = self.next_original_check.min(now + ORIGINAL_EVERY);
                }
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
        let deciding = self.deciding(direction);
        self.placed
            .get_mut(direction)
            .keep_current(&current, |role| deciding.contains(&role));
        // What a new claim finds on the endpoint is FxSound's to adopt — on its driver, which
        // nothing plays while neither FxSound runs, where the record says a run left it, or where
        // Windows put it by itself — and otherwise the user's own: a cable they made their default
        // (`Claim::finding_theirs`; module docs, "Whose default it is").
        let adopts = self.is_driver(&endpoint)
            || self.claimed.get(direction).as_deref() == Some(endpoint.as_str());
        let mut theirs = Defaults::default();
        for role in Role::ALL {
            if !adopts
                && current.get(role) == Some(endpoint.as_str())
                && !self.placed_by_windows(direction, role, &endpoint)
            {
                theirs.set(role, Some(endpoint.clone()));
            }
        }
        let lane = self.lanes.get_mut(direction);
        lane.reclaim = false;
        let mut claim = lane
            .claim
            .take()
            .filter(|claim| claim.endpoint == endpoint)
            .unwrap_or_else(|| Claim::finding_theirs(endpoint.clone(), &theirs));
        let roles = defaults::claim_roles(
            &current,
            &endpoint,
            Role::claimed_by(direction),
            &mut claim,
            |role, now| self.remembered_before(direction, role, now),
        );
        // Written down before the first write, so that a run killed at any point after it is
        // known to have left the default here (`record`) — and only for a claim that holds
        // something: one whose every role was the user's own already leaves nothing behind.
        if !roles.is_empty() || claim.holds(&current) {
            self.keep_claimed(direction, Some(&endpoint));
        }
        if let Some(role) = Role::ALL.into_iter().find(|&role| claim.is_theirs(role)) {
            log::info!(
                "{} lane: the {role:?} default was on {endpoint} already, the user's own; it is left \
                 there",
                direction.key()
            );
        }
        if let Some(role) = Role::ALL.into_iter().find(|&role| {
            current.get(role) == Some(endpoint.as_str())
                && self.placed_by_windows(direction, role, &endpoint)
        }) {
            log::info!(
                "{} lane: Windows had put the {role:?} default on {endpoint} by itself; FxSound \
                 holds it, and gives it back to a real device",
                direction.key()
            );
        }
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
        if direction == DeviceDirection::Input {
            self.free_the_microphone_s_cable();
        }
    }

    /// What a claim of the lane of `direction` remembers as `role`'s default from before FxSound,
    /// that default being `now` (`defaults::claim_roles`): `now` itself, for a device of the
    /// user's; nothing, for one of FxSound's own; and for one Windows put on a cable by itself,
    /// where the role was before that, while it is a device of the user's still there — so that
    /// neither the settings ([`AudioToUi::RememberedDefault`]) nor the hand-back ever name the
    /// cable (module docs, "Whose default it is").
    fn remembered_before(
        &self,
        direction: DeviceDirection,
        role: Role,
        now: &str,
    ) -> Option<String> {
        if self.placed_by_windows(direction, role, now) {
            return self
                .placed
                .get(direction)
                .get(role)
                .and_then(|placed| placed.before.clone())
                .filter(|before| self.is_listed(direction, before) && !self.is_virtual(before));
        }
        (!self.is_virtual(now)).then(|| now.to_owned())
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
    ///
    /// Never back onto the claim's own device: a cable no lane stands on is a device like any
    /// other, but the one a default is being given back from — a killed run's, which no lane
    /// stands on any more — is the device it is leaving.
    fn give_back(&mut self, direction: DeviceDirection, claim: &Claim) {
        let current = self.system.defaults(direction);
        *self.defaults.get_mut(direction) = current.clone();
        let leaving = endpoints::device_of(&claim.endpoint, &self.endpoints);
        let elsewhere = |id: &str| !leaving.iter().any(|side| side == id);
        let device = self.give_back_device(direction, &leaving);
        let writes = defaults::hand_back(&current, claim, device.as_deref(), |id| {
            self.is_listed(direction, id) && elsewhere(id)
        });
        for (role, target) in writes {
            self.write_default(direction, &target, role);
        }
        if claim.holds(self.defaults.get(direction)) {
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

    /// The device a default of `direction` goes back to when it leaves the endpoints `leaving`
    /// (`devices::restore_default_candidate`): the one the rules remember first — the user's pick,
    /// the device the lane played to, a default from before FxSound — and with nothing remembered
    /// here, the device the lane is on, or failing that the first real device. A default left on a
    /// virtual device swallows every sound, which is worse than any real device; and none of these
    /// is ever a cable the user has not chosen ([`Self::device_infos`]), Windows having made it the
    /// default or not.
    fn give_back_device(&self, direction: DeviceDirection, leaving: &[String]) -> Option<String> {
        let elsewhere = |id: &str| !leaving.iter().any(|side| side == id);
        let infos: Vec<DeviceInfo> = self
            .device_infos()
            .into_iter()
            .filter(|info| elsewhere(&info.name))
            .collect();
        devices::restore_default_candidate(self.memory.get(direction), &infos, direction).or_else(
            || {
                self.lanes
                    .get(direction)
                    .target
                    .clone()
                    .filter(|target| self.is_listed(direction, target) && elsewhere(target))
                    .or_else(|| {
                        infos
                            .iter()
                            .find(|device| device.direction == direction)
                            .map(|device| device.name.clone())
                    })
            },
        )
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
                // FxSound's own write, wherever Windows had put the role before.
                self.placed.get_mut(direction).set(role, None);
                self.found.get_mut(direction).set(role, None);
                // What Windows is about to report, known now: a change heard before its echo is
                // a change from here.
                if let Some(reported) = self.reported.as_mut() {
                    reported.get_mut(direction).set(role, Some(id.to_owned()));
                }
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
    /// does not name had its default put there by the user, or by Windows, and never by FxSound:
    /// it is left alone.
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
