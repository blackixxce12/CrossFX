//! A lane's stream thread: the capture stream, the lane's chain, the ring and the render stream,
//! on one thread registered with MMCSS.
//!
//! ```text
//!  output lane:  virtual device ──loopback──► chain ──► ring ──► real device
//!  input lane:   microphone ─────capture────► chain ──► ring ──► cable's playback side
//! ```
//!
//! # One thread, two streams
//!
//! The original ran its capture and its render in one loop on one thread, polling both with
//! `Sleep(1)` (`docs/spec/12-audio-io.md` §10). Here the loop waits for the render stream's event
//! — WASAPI signals it every time the device has taken a period — and for the capture stream's,
//! when it has one, and on every wake takes every packet the capture side has, runs it through the
//! chain into the ring, and tops the render buffer up from the ring ([`super::ring`]). Both ends of
//! the ring are on this thread, so it is a plain buffer, and the chain runs on the capture side, as
//! NODE 1's `process()` runs it on Linux: an underrun plays silence rather than processing stale
//! samples twice.
//!
//! # Idle
//!
//! The original stopped its render stream whenever its ring ran dry, "to allow PC to sleep if no
//! audio is playing" (`sndDevicesDoCapture.cpp:137`). So does this, a second after the last packet
//! that held a sound ([`IDLE_AFTER`]): the chain has rung out by then, and a render stream that
//! keeps running keeps the speakers — a Bluetooth headset's radio, a USB DAC — awake for nothing.
//! The capture stream keeps running, and the first packet with a sound in it starts the lane again:
//! the ring primes afresh, the chain fades in from silence, as a new pair's does on Linux, and the
//! render stream is started once it has something to play.
//!
//! Idle, the thread itself rests too. With the render stream stopped, nothing playing into the
//! virtual device and the capture side event-driven, no event comes, and a 10 ms poll would wake
//! it 64 to 100 times a second for nothing. It waits instead for a packet, for a request of the
//! control thread's — which wakes it through an event of its own ([`StreamShared`]) — or for
//! [`IDLE_WAIT`], whichever comes first: the timeout is how an idle lane still notices that its
//! streams died under it.
//!
//! A sound does not always end in silent packets. A loopback stream delivers no packets at all
//! while nothing plays into the virtual device — the live check reads none in 200 ms of an idle
//! "FxSound Speakers" (`super::live`) — so an application that stops its stream leaves the output
//! lane's capture side with nothing to read, where a PipeWire sink would be handed silence. Once
//! it has been quiet for two packets ([`super::ring::quiet_after`]) the source is taken to have
//! stopped, and the ring is kept at its fill with silence run through the chain
//! ([`super::ring::silence_lacking`]): the chain's tail rings out instead of being cut off, the
//! meters fall, the fade of a stop asked for meanwhile is played, and nothing is counted as an
//! underrun. The second without a sound runs from the last packet that held one, whether silent
//! packets or none followed it. Without this the lane never went idle once an application had
//! stopped its stream, and counted every pause as an underrun.
//!
//! # Stopping without a click
//!
//! A lane that stops while it plays fades out first: the control thread asks for it
//! ([`StreamShared::fade_out`]), the chain falls silent through the lane DSP's own mute fade
//! (`crate::lane_dsp`, `MUTE_FADE_SECONDS`), and the thread ends once that silence has reached the
//! device. A lane handing its default back keeps playing for a moment first (`tail`), while the
//! applications move to the device the default went to — what they played into the virtual device
//! meanwhile is still heard. A stream that starts fades in: the lane DSP's `begin_pair`. A stream
//! that fails — its device unplugged — stops where it is: there is nothing left to fade on.
//!
//! # Real-time rules
//!
//! Inside the loop nothing allocates or locks: the ring, the chain's scratch and the block of
//! silence are allocated before it, the control thread is heard through atomics, and the only
//! calls out are WASAPI's own. Everything that may block — opening the streams, telling the control
//! thread the stream ended — happens before the loop or after it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use fxsound_core::DeviceDirection;

use super::Event;
use super::com::{
    self, Apartment, CaptureStream, Packet, ProAudio, RenderStream, Source, WaitEvent, Woken,
};
use super::format::StreamFormat;
use super::ring::{DriftControl, FrameRing, Targets, quantum_frames, quiet_after, silence_lacking};
use crate::lane_dsp::{LaneDsp, MUTE_FADE_SECONDS};
use crate::{AudioError, LANE_CHANNELS, RING_CAPACITY_FRAMES};

/// How long a stream may take to open before the control thread gives up on it: an endpoint that
/// takes longer is not going to work.
const OPEN_TIMEOUT: Duration = Duration::from_secs(3);

/// How long a stream may take to fade out and stop. A thread that has not ended by then is stuck
/// in a call to WASAPI; it is left to end on its own, and its DSP with it.
const STOP_TIMEOUT: Duration = Duration::from_secs(2);

/// The longest a fade out may run, its tail included.
const FADE_LIMIT: Duration = Duration::from_millis(400);

/// How long after the last packet with a sound in it the lane goes idle (module docs, "Idle"):
/// the Linux engine's `SLEEP_AFTER`.
const IDLE_AFTER: Duration = Duration::from_secs(1);

/// The longest the thread waits for an event before it looks again: a stream that is not
/// event-driven is read this often, and so is one with a fade or a quiet source to keep up.
const POLL: Duration = Duration::from_millis(10);

/// The longest an idle lane waits (module docs, "Idle"): with the render stream stopped and an
/// event-driven capture side, nothing is signalled until a packet arrives or the control thread
/// asks for something — which sets [`StreamShared`]'s wake — so the wait could be endless. It is
/// not: the timeout is also how an idle lane learns that its streams died, from the capture
/// side's next read (`AUDCLNT_E_DEVICE_INVALIDATED`, the audio service gone), which the engine's
/// recovery waits for. The supervisor's own beat (`super::SUPERVISOR_PERIOD`): five wakes a second
/// instead of the hundred, or the 64 of the system timer, a 10 ms poll costs.
const IDLE_WAIT: Duration = Duration::from_millis(200);

/// How long the stream thread waits for its events: [`IDLE_WAIT`] while the lane is idle with its
/// render stream stopped and everything that can wake it event-driven — the capture side's
/// packets, the control thread's requests (`woken`) — and [`POLL`] otherwise.
const fn wait_for(idle: bool, playing: bool, evented: bool, woken: bool) -> Duration {
    if idle && !playing && evented && woken {
        IDLE_WAIT
    } else {
        POLL
    }
}

/// Which device a lane's stream runs on, and for which lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StreamPlan {
    pub(crate) direction: DeviceDirection,
    /// The endpoint captured: the virtual device (looped back) for the output lane, the
    /// microphone for the input lane.
    pub(crate) capture: String,
    /// The endpoint rendered to: the real device for the output lane, the cable's playback side
    /// for the input lane.
    pub(crate) render: String,
    /// Which of the lane's streams this is: the control thread's count, so that the end of a
    /// stream it has already replaced is told from the end of the one it runs.
    pub(crate) generation: u64,
}

impl StreamPlan {
    const fn source(&self) -> Source {
        match self.direction {
            DeviceDirection::Output => Source::Loopback,
            DeviceDirection::Input => Source::Microphone,
        }
    }
}

/// What a stream opened at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Opened {
    pub(crate) format: StreamFormat,
    /// The capture stream's packet, in frames.
    pub(crate) packet: usize,
    /// The render stream's period, in frames.
    pub(crate) period: usize,
    pub(crate) targets: Targets,
}

/// How a stream ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Asked to, and did.
    Stopped,
    /// A device of the stream went away under it (`AUDCLNT_E_DEVICE_INVALIDATED`): the endpoints
    /// are read again and the lane's rules run, as on Linux when a target's node goes.
    DeviceGone,
    /// The audio service went away.
    ServiceGone(String),
    /// Anything else.
    Failed(AudioError),
}

/// A stream's thread as it ended: the lane's DSP, handed back, why it ended, and what it counted.
pub(crate) struct Ended {
    /// `None` for a thread that never received it, and for one [`Self::left_behind`].
    pub(crate) dsp: Option<LaneDsp>,
    pub(crate) outcome: Outcome,
    pub(crate) totals: Totals,
    /// The thread, when it did not end in time ([`STOP_TIMEOUT`]): stuck in a call to WASAPI —
    /// an endpoint that takes seconds to open, a Bluetooth headset between profiles — with the
    /// lane's DSP still on it. The control thread does not wait for it; it takes the DSP back once
    /// the thread has ended ([`LeftBehind::collect`]), and the lane runs again.
    pub(crate) left_behind: Option<LeftBehind>,
}

impl Ended {
    /// A thread that ended, or never started, holding `dsp`.
    pub(crate) fn with(dsp: Option<LaneDsp>, outcome: Outcome) -> Self {
        Self {
            dsp,
            outcome,
            totals: Totals::default(),
            left_behind: None,
        }
    }
}

/// Why a stream could not be started, with what became of the lane's DSP ([`Stream::start`]):
/// boxed, because the DSP it may carry back is large and the success path should not pay for it.
pub(crate) type StartFailure = (Box<Ended>, AudioError);

/// A stream's thread that was asked to end and did not in time ([`Ended::left_behind`]). It has
/// been told to end at once, and does as soon as WASAPI gives it back.
pub(crate) struct LeftBehind {
    join: JoinHandle<Ended>,
    shared: Arc<StreamShared>,
}

impl LeftBehind {
    /// Whether the thread has ended, and [`Self::collect`] will not wait.
    pub(crate) fn has_ended(&self) -> bool {
        self.join.is_finished()
    }

    /// What the thread handed back as it ended: the lane's DSP above all.
    pub(crate) fn collect(self) -> Ended {
        let mut ended = joined(self.join);
        ended.totals = Totals::of(&self.shared);
        ended
    }

    /// A thread left behind, for the engine's tests: one that ends when the test lets it.
    #[cfg(test)]
    pub(crate) fn for_tests(join: JoinHandle<Ended>) -> Self {
        Self {
            join,
            shared: Arc::new(StreamShared::new(false, 1.0)),
        }
    }
}

/// What a stream counted: the Linux ring's counters, which the lane sums over all its streams.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Totals {
    pub(crate) frames: u64,
    pub(crate) dropped: u64,
    pub(crate) underruns: u64,
    pub(crate) resyncs: u64,
}

impl Totals {
    pub(crate) fn of(shared: &StreamShared) -> Self {
        Self {
            frames: shared.frames.load(Ordering::Relaxed),
            dropped: shared.dropped.load(Ordering::Relaxed),
            underruns: shared.underruns.load(Ordering::Relaxed),
            resyncs: shared.resyncs.load(Ordering::Relaxed),
        }
    }

    pub(crate) const fn plus(self, other: Self) -> Self {
        Self {
            frames: self.frames + other.frames,
            dropped: self.dropped + other.dropped,
            underruns: self.underruns + other.underruns,
            resyncs: self.resyncs + other.resyncs,
        }
    }
}

/// What the control thread asks of a running stream.
const RUN: u8 = 0;
const FADE: u8 = 1;
const NOW: u8 = 2;

/// Everything the control thread and a stream's thread share: atomics, written by one side and
/// read by the other once a block, and the event that wakes the stream thread for a request.
#[derive(Debug)]
pub(crate) struct StreamShared {
    request: AtomicU8,
    /// Set with every request ([`Self::fade_out`], [`Self::end_now`]), so that a thread waiting
    /// out an idle lane's long wait ([`IDLE_WAIT`]) hears it at once. `None` when the system had
    /// no event to give, and the thread then never waits longer than [`POLL`].
    wake: Option<WaitEvent>,
    /// For a fade: how long the stream plays on before it falls silent, in milliseconds.
    tail_ms: AtomicU32,
    /// The engine holds the lane silent while the system sleeps (`crate::lane_dsp`, "Mute").
    system_mute: AtomicBool,
    /// The virtual device's own volume as an amplitude (`f32` bits): the output lane plays at it,
    /// after the chain, as the Linux lanes play at their node's volume.
    gain: AtomicU32,
    /// Blocks with something in them run through the chain: what the control thread reads as
    /// `AudioStatus::processing`.
    pub(crate) cycles: AtomicU64,
    /// Frames run through the chain.
    pub(crate) frames: AtomicU64,
    /// Frames the ring had no room for.
    pub(crate) dropped: AtomicU64,
    /// Frames of silence played because the ring ran dry.
    pub(crate) underruns: AtomicU64,
    /// Times the ring was primed again after running dry: each one an audible gap.
    pub(crate) resyncs: AtomicU64,
}

impl StreamShared {
    pub(crate) fn new(system_mute: bool, gain: f32) -> Self {
        Self {
            request: AtomicU8::new(RUN),
            wake: WaitEvent::new()
                .map_err(|error| log::debug!("no wake event for the stream thread: {error}"))
                .ok(),
            tail_ms: AtomicU32::new(0),
            system_mute: AtomicBool::new(system_mute),
            gain: AtomicU32::new(gain.to_bits()),
            cycles: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
            resyncs: AtomicU64::new(0),
        }
    }

    /// Ask the stream to fade out and end, after playing on for `tail`.
    fn fade_out(&self, tail: Duration) {
        self.tail_ms.store(
            u32::try_from(tail.as_millis()).unwrap_or(u32::MAX),
            Ordering::Relaxed,
        );
        let _ = self
            .request
            .compare_exchange(RUN, FADE, Ordering::Release, Ordering::Relaxed);
        self.wake();
    }

    /// Ask the stream to end at once.
    fn end_now(&self) {
        self.request.store(NOW, Ordering::Release);
        self.wake();
    }

    /// Wake the stream thread out of its wait, after a request has been stored.
    fn wake(&self) {
        if let Some(wake) = self.wake.as_ref() {
            wake.set();
        }
    }

    pub(crate) fn set_system_mute(&self, muted: bool) {
        self.system_mute.store(muted, Ordering::Relaxed);
    }

    pub(crate) fn set_gain(&self, gain: f32) {
        let gain = if gain.is_finite() {
            gain.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.gain.store(gain.to_bits(), Ordering::Relaxed);
    }

    pub(crate) fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Relaxed))
    }

    #[cfg(test)]
    pub(crate) fn system_muted(&self) -> bool {
        self.system_mute.load(Ordering::Relaxed)
    }
}

/// A lane's running stream, as the control thread holds it.
pub(crate) struct Stream {
    pub(crate) plan: StreamPlan,
    pub(crate) shared: Arc<StreamShared>,
    pub(crate) opened: Opened,
    join: JoinHandle<Ended>,
    /// Disconnected once the thread has ended: its sender is the thread's, dropped as it returns.
    done: Receiver<()>,
}

impl Stream {
    /// Start a stream on `plan`'s devices with the lane's `dsp`, silent from its first block when
    /// `system_mute`, at `gain`. Returns once both streams are open and the capture side runs, or
    /// with the reason they could not be opened and what became of the DSP: back in hand, or still
    /// on a thread that did not end in time ([`Ended::left_behind`]).
    ///
    /// The DSP goes over a channel rather than into the thread's closure, so that it is still in
    /// hand when the thread cannot be spawned.
    pub(crate) fn start(
        plan: StreamPlan,
        dsp: LaneDsp,
        events: Sender<Event>,
        system_mute: bool,
        gain: f32,
    ) -> Result<Self, StartFailure> {
        let shared = Arc::new(StreamShared::new(system_mute, gain));
        let (dsp_tx, dsp_rx) = crossbeam_channel::bounded::<LaneDsp>(1);
        let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
        let (done_tx, done) = crossbeam_channel::bounded::<()>(0);
        let thread_plan = plan.clone();
        let thread_shared = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name(format!("crossfx-{}", plan.direction.key()))
            .spawn(move || {
                let _done = done_tx;
                run(&thread_plan, &dsp_rx, &thread_shared, &ready_tx, &events)
            });
        let join = match spawned {
            Ok(join) => join,
            Err(error) => {
                let error = AudioError::WindowsAudioUnavailable(format!(
                    "could not start the audio thread: {error}"
                ));
                return Err((
                    Box::new(Ended::with(Some(dsp), Outcome::Failed(error.clone()))),
                    error,
                ));
            }
        };
        if let Err(crossbeam_channel::SendError(dsp)) = dsp_tx.send(dsp) {
            let _ = join.join();
            let error = AudioError::WindowsAudioUnavailable(
                "the audio thread ended before it started".to_owned(),
            );
            return Err((
                Box::new(Ended::with(Some(dsp), Outcome::Failed(error.clone()))),
                error,
            ));
        }
        match ready_rx.recv_timeout(OPEN_TIMEOUT) {
            Ok(Ok(opened)) => Ok(Self {
                plan,
                shared,
                opened,
                join,
                done,
            }),
            Ok(Err(error)) => Err((Box::new(finish(join, &done, &shared, STOP_TIMEOUT)), error)),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                log::warn!(
                    "{} lane: the streams took longer than {OPEN_TIMEOUT:?} to open",
                    plan.direction.key()
                );
                shared.end_now();
                Err((
                    Box::new(finish(join, &done, &shared, STOP_TIMEOUT)),
                    AudioError::DeviceUnavailable,
                ))
            }
        }
    }

    /// Whether the thread has ended — on its own, when the control thread did not ask.
    pub(crate) fn has_ended(&self) -> bool {
        self.join.is_finished()
    }

    /// Fade the stream out, after `tail` of playing on, and wait for it to end.
    pub(crate) fn stop(self, tail: Duration) -> Ended {
        self.shared.fade_out(tail);
        self.finish()
    }

    /// Collect a stream that ended on its own.
    pub(crate) fn collect(self) -> Ended {
        self.shared.end_now();
        self.finish()
    }

    fn finish(self) -> Ended {
        let Self {
            shared, join, done, ..
        } = self;
        let mut ended = finish(join, &done, &shared, STOP_TIMEOUT);
        if ended.left_behind.is_none() {
            ended.totals = Totals::of(&shared);
        }
        ended
    }
}

/// Wait for a stream's thread to end, and take what it hands back. One that does not end within
/// `patience` ([`STOP_TIMEOUT`]) is asked to end at once and given as long again; one stuck past
/// that is left behind ([`Ended::left_behind`]) — a hang in WASAPI must not hang the engine, nor
/// take the lane's DSP with it for good.
fn finish(
    join: JoinHandle<Ended>,
    done: &Receiver<()>,
    shared: &Arc<StreamShared>,
    patience: Duration,
) -> Ended {
    let ended = matches!(
        done.recv_timeout(patience),
        Err(RecvTimeoutError::Disconnected)
    ) || {
        shared.end_now();
        matches!(
            done.recv_timeout(patience),
            Err(RecvTimeoutError::Disconnected)
        )
    };
    if !ended {
        log::error!(
            "a CrossFX audio thread is stuck in WASAPI; leaving it to end on its own, the lane's \
             DSP with it until it does"
        );
        let mut ended = Ended::with(None, Outcome::Failed(AudioError::DeviceUnavailable));
        ended.left_behind = Some(LeftBehind {
            join,
            shared: Arc::clone(shared),
        });
        return ended;
    }
    joined(join)
}

/// What a thread that has ended handed back.
fn joined(join: JoinHandle<Ended>) -> Ended {
    join.join().unwrap_or_else(|_| {
        log::error!("a CrossFX audio thread panicked");
        Ended::with(None, Outcome::Failed(AudioError::DeviceUnavailable))
    })
}

/// The thread's body: take the DSP, run the stream, say it ended, hand the DSP back.
fn run(
    plan: &StreamPlan,
    dsp: &Receiver<LaneDsp>,
    shared: &StreamShared,
    ready: &Sender<Result<Opened, AudioError>>,
    events: &Sender<Event>,
) -> Ended {
    let Ok(mut dsp) = dsp.recv() else {
        return Ended::with(None, Outcome::Stopped);
    };
    let outcome = stream(plan, &mut dsp, shared, ready);
    match &outcome {
        Outcome::Stopped => {}
        other => log::info!("{} lane's stream ended: {other:?}", plan.direction.key()),
    }
    let _ = events.send(Event::StreamEnded {
        direction: plan.direction,
        generation: plan.generation,
    });
    Ended::with(Some(dsp), outcome)
}

/// Open the two streams, tell the control thread how that went, and run them.
fn stream(
    plan: &StreamPlan,
    dsp: &mut LaneDsp,
    shared: &StreamShared,
    ready: &Sender<Result<Opened, AudioError>>,
) -> Outcome {
    // Declared first, so dropped last: every COM object below is released before the thread
    // leaves the apartment.
    let _apartment = match Apartment::join() {
        Ok(apartment) => apartment,
        Err(error) => {
            let error = AudioError::WindowsAudioUnavailable(error.message());
            let _ = ready.send(Err(error.clone()));
            return Outcome::Failed(error);
        }
    };
    let (capture, render) = match open(plan) {
        Ok(streams) => streams,
        Err(error) => {
            let _ = ready.send(Err(error.clone()));
            return Outcome::Failed(error);
        }
    };
    let format = capture.format();
    let channels = usize::from(format.channels);
    let targets = Targets::new(
        quantum_frames(format.rate),
        capture.period(),
        render.period(),
    );
    dsp.set_format(format.rate as f32, channels);
    // The subwoofer, the front pair and the side of every channel, from the channel mask: the
    // music chain's business, ignored by the voice chain's lane.
    dsp.set_layout(&format.layout());
    // What the microphone really carries, recorded at 48 kHz whatever it is: the de-esser's
    // business, ignored by the music chain's lane.
    dsp.set_source_rate(
        capture
            .native_rate()
            .filter(|&rate| rate != format.rate)
            .map(|rate| rate as f32),
    );
    // The stream starts at the virtual device's volume, faded in from silence: the first block is
    // already at it rather than ramped there from unity.
    dsp.set_volume(&[shared.gain(); LANE_CHANNELS]);
    dsp.begin_pair();
    let opened = Opened {
        format,
        packet: capture.period(),
        period: render.period(),
        targets,
    };
    if let Err(error) = capture.start() {
        let error = open_error(&error);
        let _ = ready.send(Err(error.clone()));
        return Outcome::Failed(error);
    }
    log::info!(
        "{} lane: {} → {}, {} ch @ {} Hz, packets of {} frames, periods of {}, ring at {} ± {}, \
         the chain {} frames late",
        plan.direction.key(),
        plan.capture,
        plan.render,
        format.channels,
        format.rate,
        opened.packet,
        opened.period,
        targets.ring,
        targets.band,
        dsp.latency_frames()
    );
    let _ = ready.send(Ok(opened));
    let pro_audio = ProAudio::join();
    let started = Instant::now();
    let mut lane = Pump {
        // The Linux ring's size, or room for twice the priming fill of a device whose periods are
        // long enough to need more: a ring the fill it is primed at does not fit in never plays.
        ring: FrameRing::new(
            channels,
            RING_CAPACITY_FRAMES.max(targets.prime().saturating_mul(2)),
        ),
        drift: DriftControl::new(&targets),
        targets,
        silence: vec![0; dsp.block_bytes()],
        channels,
        block_align: format.block_align(),
        rate: format.rate,
        primed: false,
        fading_in: false,
        playing: false,
        idle: true,
        last_sound: started,
        last_packet: started,
        quiet_after: quiet_after(opened.packet, format.rate),
        gains: [1.0; LANE_CHANNELS],
        muting: false,
    };
    let outcome = lane.run(dsp, shared, &capture, &render);
    render.stop();
    capture.stop();
    drop(pro_audio);
    log::info!(
        "{} lane: stream stopped; {} drift corrections, {} frames dropped, {} of silence played \
         short",
        plan.direction.key(),
        lane.drift.corrections(),
        shared.dropped.load(Ordering::Relaxed),
        shared.underruns.load(Ordering::Relaxed)
    );
    outcome
}

/// Open the capture stream and the render stream of `plan`, the render stream at the capture
/// stream's format.
fn open(plan: &StreamPlan) -> Result<(CaptureStream, RenderStream), AudioError> {
    let enumerator = com::Enumerator::new()
        .map_err(|error| AudioError::WindowsAudioUnavailable(error.message()))?;
    let capture_device = enumerator
        .device(&plan.capture)
        .map_err(|error| open_error(&error))?;
    let render_device = enumerator
        .device(&plan.render)
        .map_err(|error| open_error(&error))?;
    let refused = |id: &str, what: &str, error: &windows_core::Error| {
        if com::is_device_in_use(error) {
            log::warn!("{id}: no {what} stream: another application holds it in exclusive mode");
        } else {
            log::warn!("{id}: no {what} stream: {error}");
        }
        open_error(error)
    };
    let capture = CaptureStream::open(&capture_device, plan.source())
        .map_err(|error| refused(&plan.capture, "capture", &error))?;
    let render = RenderStream::open(&render_device, &capture.format())
        .map_err(|error| refused(&plan.render, "render", &error))?;
    Ok((capture, render))
}

/// The [`AudioError`] a stream that could not be opened is reported as: the original's
/// `AUDCLNT_E_DEVICE_IN_USE` / `AUDCLNT_E_UNSUPPORTED_FORMAT` latch (`sndDevicesSetupDevices.cpp:500`)
/// split into the two errors the GUI already names.
fn open_error(error: &windows_core::Error) -> AudioError {
    if com::is_device_gone(error) {
        AudioError::DeviceNotPresent
    } else if com::is_format_refused(error) {
        AudioError::FormatNegotiation
    } else if com::is_service_gone(error) {
        AudioError::WindowsAudioUnavailable(error.message())
    } else {
        AudioError::DeviceUnavailable
    }
}

/// How a stream that failed while it ran ended.
fn failure(error: &windows_core::Error) -> Outcome {
    if com::is_device_gone(error) {
        Outcome::DeviceGone
    } else if com::is_service_gone(error) {
        Outcome::ServiceGone(error.message())
    } else {
        Outcome::Failed(AudioError::DeviceUnavailable)
    }
}

/// Whether a block of little-endian `f32` holds anything but silence.
fn holds_sound(bytes: &[u8]) -> bool {
    bytes
        .as_chunks::<{ size_of::<f32>() }>()
        .0
        .iter()
        .any(|word| f32::from_le_bytes(*word) != 0.0)
}

/// The loop's state: everything the stream thread keeps between two wakes.
struct Pump {
    ring: FrameRing,
    drift: DriftControl,
    targets: Targets,
    /// A block of silence, for packets the capture stream marks silent.
    silence: Vec<u8>,
    channels: usize,
    block_align: usize,
    rate: u32,
    /// The ring has reached its target since it last ran dry.
    primed: bool,
    /// The ring is being primed with the start of a sound, which the chain fades in from silence:
    /// none of it is dropped to reach the target.
    fading_in: bool,
    /// The render stream has been started.
    playing: bool,
    /// Nothing has been heard for [`IDLE_AFTER`] (module docs, "Idle").
    idle: bool,
    /// When the capture side last delivered a packet that held a sound.
    last_sound: Instant,
    /// When the capture side last delivered a packet at all, sound or silence.
    last_packet: Instant,
    /// How long without a packet the source is taken to have stopped (module docs, "Idle").
    quiet_after: Duration,
    gains: [f32; LANE_CHANNELS],
    /// A fade out has begun: the chain is silent from here on.
    muting: bool,
}

impl Pump {
    fn run(
        &mut self,
        dsp: &mut LaneDsp,
        shared: &StreamShared,
        capture: &CaptureStream,
        render: &RenderStream,
    ) -> Outcome {
        let mut fade: Option<Fade> = None;
        loop {
            let now = Instant::now();
            match shared.request.load(Ordering::Acquire) {
                NOW => return Outcome::Stopped,
                FADE => {
                    let fade = fade.get_or_insert_with(|| Fade {
                        mute_at: now
                            + Duration::from_millis(u64::from(
                                shared.tail_ms.load(Ordering::Relaxed),
                            )),
                        done_at: None,
                        limit: now + FADE_LIMIT,
                    });
                    if self.idle || now >= fade.limit {
                        return Outcome::Stopped;
                    }
                    if !self.muting && now >= fade.mute_at {
                        self.muting = true;
                        // What is queued now is the last of the unmuted sound; the fade follows
                        // it, and a period after that the device has played it all.
                        let queued = self.ring.len() + render.padding().unwrap_or(0);
                        let queued = Duration::from_secs_f64(queued as f64 / f64::from(self.rate));
                        fade.done_at = Some(
                            now + queued + Duration::from_secs_f32(MUTE_FADE_SECONDS) + POLL * 2,
                        );
                    }
                    if fade.done_at.is_some_and(|done| now >= done) {
                        return Outcome::Stopped;
                    }
                }
                _ => {}
            }

            // The render stream's event, the capture stream's when it has one, and the control
            // thread's wake: an idle lane waits on them for long (`IDLE_WAIT`), any other for a
            // poll's length at most.
            let timeout = wait_for(
                self.idle,
                self.playing,
                capture.event().is_some(),
                shared.wake.is_some(),
            );
            let woken = match (capture.event(), shared.wake.as_ref()) {
                (Some(packets), Some(wake)) => {
                    com::wait_any(&[render.event(), packets, wake], timeout)
                }
                (Some(packets), None) => com::wait_any(&[render.event(), packets], timeout),
                (None, Some(wake)) => com::wait_any(&[render.event(), wake], timeout),
                (None, None) => com::wait_any(&[render.event()], timeout),
            };
            if woken == Woken::Failed {
                return Outcome::Failed(AudioError::DeviceUnavailable);
            }

            let now = Instant::now();
            let muted = shared.system_mute.load(Ordering::Relaxed) || self.muting;
            let gain = shared.gain();
            self.gains.fill(gain);
            let taken = capture.drain(|packet| self.take(dsp, shared, packet, now, muted));
            if let Err(error) = taken {
                return failure(&error);
            }
            // A second without a sound, whether silent packets came meanwhile or none did
            // (module docs, "Idle").
            if !self.idle && now.saturating_duration_since(self.last_sound) >= IDLE_AFTER {
                self.idle = true;
            }
            if self.idle {
                // The newest snapshot and every event all the same, so that neither waits for
                // the next sound: the GUI's events would pile up against their queue's length.
                dsp.refresh();
            }
            if let Err(error) = self.render(render, shared, dsp, now, muted) {
                return failure(&error);
            }
        }
    }

    /// One packet from the capture stream: through the chain and into the ring, unless the lane is
    /// idle and the packet holds nothing.
    fn take(
        &mut self,
        dsp: &mut LaneDsp,
        shared: &StreamShared,
        packet: Packet<'_>,
        now: Instant,
        muted: bool,
    ) {
        self.last_packet = now;
        let (frames, sound) = match packet {
            Packet::Samples(bytes) => (bytes.len() / self.block_align, holds_sound(bytes)),
            Packet::Silence(frames) => (frames, false),
        };
        if sound {
            if self.idle {
                // The first sound after the lane stood still comes in from silence, as a new
                // pair's does — and the ring is primed with the start of that fade in it.
                self.idle = false;
                self.fading_in = true;
                dsp.fade_in();
            }
            self.last_sound = now;
        }
        if self.idle {
            return;
        }
        let dropped = self.process(dsp, packet, muted);
        shared.cycles.fetch_add(1, Ordering::Relaxed);
        shared.frames.fetch_add(frames as u64, Ordering::Relaxed);
        if dropped > 0 {
            shared.dropped.fetch_add(dropped as u64, Ordering::Relaxed);
        }
    }

    /// Run `packet` through the chain into the ring, and publish the chain's meters. Returns how
    /// many frames the ring had no room for.
    fn process(&mut self, dsp: &mut LaneDsp, packet: Packet<'_>, muted: bool) -> usize {
        // Parameters are state: the newest snapshot. Events are not: all of them. The volume and
        // the mute are read once a block too.
        dsp.refresh();
        dsp.set_volume(&self.gains);
        dsp.set_system_mute(muted);

        let block = (dsp.block_bytes() / self.block_align).max(1) * self.block_align;
        let mut dropped = 0;
        match packet {
            Packet::Samples(bytes) => {
                for chunk in bytes.chunks(block) {
                    let Some(processed) = dsp.process_bytes(chunk, self.channels) else {
                        break;
                    };
                    dropped += self.ring.push(processed);
                }
            }
            Packet::Silence(frames) => {
                let mut left = frames * self.block_align;
                while left > 0 {
                    let length = left.min(block).min(self.silence.len());
                    let Some(chunk) = self.silence.get(..length).filter(|chunk| !chunk.is_empty())
                    else {
                        break;
                    };
                    let Some(processed) = dsp.process_bytes(chunk, self.channels) else {
                        break;
                    };
                    dropped += self.ring.push(processed);
                    left -= length;
                }
            }
        }
        dsp.publish_meters();
        dropped
    }

    /// Top the render buffer up from the ring, priming first — and, with the source stopped, the
    /// ring up with silence first (module docs, "Idle"); or, idle, let the render stream stop
    /// once it has played what it had.
    fn render(
        &mut self,
        render: &RenderStream,
        shared: &StreamShared,
        dsp: &mut LaneDsp,
        now: Instant,
        muted: bool,
    ) -> windows_core::Result<()> {
        if self.idle {
            if self.playing && render.padding()? == 0 {
                render.stop();
                self.playing = false;
            }
            self.ring.clear();
            self.primed = false;
            return Ok(());
        }
        let padding = render.padding()?;
        let free = render.buffer().saturating_sub(padding);
        let want = self.targets.padding.saturating_sub(padding).min(free);
        if now.saturating_duration_since(self.last_packet) >= self.quiet_after {
            // Not counted as a block of the source's: what the chain makes of the silence after
            // it, not a packet it delivered.
            let lacking = silence_lacking(self.ring.len(), want, &self.targets, self.primed);
            if lacking > 0 {
                let dropped = self.process(dsp, Packet::Silence(lacking), muted);
                if dropped > 0 {
                    shared.dropped.fetch_add(dropped as u64, Ordering::Relaxed);
                }
            }
        }
        if !self.primed {
            // The start of a sound keeps what the ring holds, up to the drift band, so that the
            // device starts at the first frame of the chain's fade in rather than part-way up it;
            // a ring primed again after it ran dry is set to its target (`FrameRing::prime`).
            let slack = if self.fading_in { self.targets.band } else { 0 };
            if !self.ring.prime(self.targets.prime(), slack) {
                return Ok(());
            }
            self.primed = true;
            self.fading_in = false;
            self.drift.reset();
        }
        if want > 0 {
            let correction = self.drift.correction();
            let ring = &mut self.ring;
            let mut short = 0;
            render.write(want, |bytes| short = ring.pop_into(bytes, want, correction))?;
            if short > 0 {
                shared.underruns.fetch_add(short as u64, Ordering::Relaxed);
                shared.resyncs.fetch_add(1, Ordering::Relaxed);
                self.primed = false;
            } else {
                self.drift.observe(self.ring.len());
            }
        }
        if !self.playing {
            render.start()?;
            self.playing = true;
        }
        Ok(())
    }
}

/// A fade out under way.
struct Fade {
    /// When the chain falls silent: at the request, or after the tail asked for.
    mute_at: Instant,
    /// When the silence has reached the device, once the chain has fallen silent.
    done_at: Option<Instant>,
    /// When the stream ends whatever has been played.
    limit: Instant,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_is_told_from_sound_by_its_samples() {
        let silent: Vec<u8> = [0.0_f32; 8].iter().flat_map(|s| s.to_le_bytes()).collect();
        assert!(!holds_sound(&silent));
        let mut sound = silent.clone();
        sound[20..24].copy_from_slice(&1e-6_f32.to_le_bytes());
        assert!(holds_sound(&sound));
        // Negative zero is silence too.
        let negative: Vec<u8> = [-0.0_f32; 4].iter().flat_map(|s| s.to_le_bytes()).collect();
        assert!(!holds_sound(&negative));
    }

    #[test]
    fn an_idle_lane_waits_long_and_every_request_wakes_it() {
        // Idle, the render stream stopped, the capture side and the requests event-driven.
        assert_eq!(wait_for(true, false, true, true), IDLE_WAIT);
        // Anything else keeps to the poll: a sound playing, a render stream still draining, a
        // capture side read by polling, or no event for the requests.
        assert_eq!(wait_for(false, false, true, true), POLL);
        assert_eq!(wait_for(true, true, true, true), POLL);
        assert_eq!(wait_for(true, false, false, true), POLL);
        assert_eq!(wait_for(true, false, true, false), POLL);
        assert!(IDLE_WAIT <= super::super::SUPERVISOR_PERIOD);

        let shared = StreamShared::new(false, 1.0);
        let wake = shared.wake.as_ref().expect("an event to wake the thread");
        assert_eq!(com::wait_any(&[wake], Duration::ZERO), Woken::TimedOut);
        shared.fade_out(Duration::from_millis(60));
        assert_eq!(
            com::wait_any(&[wake], Duration::ZERO),
            Woken::Signalled,
            "a fade wakes the thread"
        );
        assert_eq!(
            com::wait_any(&[wake], Duration::ZERO),
            Woken::TimedOut,
            "once: the event resets itself"
        );
        shared.end_now();
        assert_eq!(
            com::wait_any(&[wake], Duration::ZERO),
            Woken::Signalled,
            "so does an end"
        );
        // A request made while the thread is not waiting is there for its next wait.
        shared.end_now();
        let started = Instant::now();
        assert_eq!(com::wait_any(&[wake], IDLE_WAIT), Woken::Signalled);
        assert!(started.elapsed() < IDLE_WAIT);
    }

    #[test]
    fn a_fade_is_asked_for_once_and_an_end_overrides_it() {
        let shared = StreamShared::new(false, 1.0);
        shared.fade_out(Duration::from_millis(60));
        assert_eq!(shared.request.load(Ordering::Relaxed), FADE);
        assert_eq!(shared.tail_ms.load(Ordering::Relaxed), 60);
        shared.end_now();
        shared.fade_out(Duration::ZERO);
        assert_eq!(
            shared.request.load(Ordering::Relaxed),
            NOW,
            "a fade does not undo an end"
        );
    }

    #[test]
    fn a_thread_stuck_past_its_time_is_left_behind_and_hands_the_dsp_back_once_it_ends() {
        let (lanes, _handover) = crate::lane_dsp::tests::lanes_for_tests();
        let dsp = lanes.output;
        let (release_tx, release) = crossbeam_channel::bounded::<()>(0);
        let (done_tx, done) = crossbeam_channel::bounded::<()>(0);
        let shared = Arc::new(StreamShared::new(false, 1.0));
        let join = std::thread::spawn(move || {
            let _done = done_tx;
            // Stuck in a call to WASAPI, as far as anyone can tell, until the test lets it go.
            let _ = release.recv();
            Ended::with(Some(dsp), Outcome::Stopped)
        });
        let ended = finish(join, &done, &shared, Duration::from_millis(20));
        assert!(ended.dsp.is_none(), "not waited for");
        assert_eq!(
            shared.request.load(Ordering::Relaxed),
            NOW,
            "told to end at once"
        );
        let thread = ended.left_behind.expect("left behind, not lost");
        drop(release_tx);
        assert!(done.recv().is_err(), "the thread ends once let go");
        let back = thread.collect();
        assert!(back.dsp.is_some(), "and the DSP comes back with it");
        assert!(back.left_behind.is_none());
    }

    #[test]
    fn the_gain_a_stream_plays_at_is_kept_to_unity_and_a_number() {
        let shared = StreamShared::new(false, 1.0);
        assert_eq!(shared.gain(), 1.0);
        shared.set_gain(0.25);
        assert_eq!(shared.gain(), 0.25);
        shared.set_gain(3.0);
        assert_eq!(shared.gain(), 1.0);
        shared.set_gain(f32::NAN);
        assert_eq!(shared.gain(), 0.0);
    }
}
