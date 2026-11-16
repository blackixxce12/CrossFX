//! The engine's rules against a system the tests describe: endpoints written out by hand, defaults
//! kept in memory, streams that hold the lane's DSP and play nothing. Nothing here touches the
//! machine's audio — no default is written, no stream opened.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::Receiver;

use super::endpoints::tests::{IN, OUT, cable, cable_a, endpoint, fxsound, microphone, speakers};
use super::format::StreamFormat;
use super::ring::Targets;
use super::*;
use crate::lane_dsp::tests::lanes_for_tests;

/// Everything the engine asked of the fake system, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Did {
    SetDefault(DeviceDirection, String, Role),
    Started(DeviceDirection, String, String),
    Stopped(DeviceDirection, Duration),
    /// The record of the defaults CrossFX holds, as kept: the output lane's, the input lane's.
    Recorded(Option<String>, Option<String>),
}

/// A running stream's switch for ending it as though by itself, and the outcome it then reports.
type EndSwitch = (Arc<AtomicBool>, Arc<std::sync::Mutex<Outcome>>);

/// The system the tests describe.
struct Fake {
    endpoints: Vec<Endpoint>,
    /// The endpoints cannot be read: the audio service is gone.
    service_gone: bool,
    defaults: PerDirection<Defaults>,
    original: Option<String>,
    refuse_streams: bool,
    gain: Option<f32>,
    /// CrossFX's record of the defaults it holds (`record`), as the last run left it.
    record: PerDirection<Option<String>>,
    /// The record's word on the defaults the user put on a cable themselves.
    users_cables: PerDirection<Defaults>,
    log: Arc<std::sync::Mutex<Vec<Did>>>,
    /// The running streams' "ended by themselves" switches, by lane.
    ended: PerDirection<Option<EndSwitch>>,
    /// Set, the next stream asked to stop sticks in WASAPI: its thread keeps the lane's DSP until
    /// the sender of this is dropped.
    stuck: Arc<std::sync::Mutex<Option<Receiver<()>>>>,
    /// How often the original FxSound was looked for.
    scans: usize,
    /// No endpoint's sessions can be watched.
    sessions_refused: bool,
    /// The endpoints whose sessions are watched.
    watched: Vec<String>,
    /// The recording endpoints another process records from.
    recorders: Vec<String>,
    /// Who records cannot be told.
    recorders_unknown: bool,
    /// The recording endpoint whose recorders are watched: none when `sessions_refused`.
    recorders_watched: Option<String>,
    /// How often a watch of recorders was made.
    recorder_watches: usize,
    /// How often who records was asked.
    recorders_asked: usize,
}

impl Fake {
    fn did(&self, what: Did) {
        if let Ok(mut log) = self.log.lock() {
            log.push(what);
        }
    }
}

struct FakeStream {
    plan: StreamPlan,
    dsp: Option<LaneDsp>,
    shared: Arc<StreamShared>,
    opened: Opened,
    ended: Arc<AtomicBool>,
    outcome: Arc<std::sync::Mutex<Outcome>>,
    log: Arc<std::sync::Mutex<Vec<Did>>>,
    stuck: Arc<std::sync::Mutex<Option<Receiver<()>>>>,
}

impl LaneStream for FakeStream {
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
        self.ended.load(Ordering::Relaxed)
    }

    fn stop(mut self: Box<Self>, tail: Duration) -> Ended {
        if let Ok(mut log) = self.log.lock() {
            log.push(Did::Stopped(self.plan.direction, tail));
        }
        let stuck = self.stuck.lock().ok().and_then(|mut stuck| stuck.take());
        if let Some(release) = stuck {
            let dsp = self.dsp.take();
            let thread = std::thread::spawn(move || {
                let _ = release.recv();
                Ended::with(dsp, Outcome::Stopped)
            });
            let mut ended = Ended::with(None, Outcome::Failed(AudioError::DeviceUnavailable));
            ended.left_behind = Some(stream::LeftBehind::for_tests(thread));
            return ended;
        }
        Ended::with(self.dsp.take(), Outcome::Stopped)
    }

    fn collect(mut self: Box<Self>) -> Ended {
        let outcome = self
            .outcome
            .lock()
            .map(|outcome| outcome.clone())
            .unwrap_or(Outcome::Stopped);
        Ended::with(self.dsp.take(), outcome)
    }
}

struct Volume(f32);

impl VolumeWatch for Volume {
    fn gain(&self) -> Option<f32> {
        Some(self.0)
    }
}

impl System for Fake {
    fn endpoints(&mut self) -> Result<Vec<Endpoint>, AudioError> {
        if self.service_gone {
            return Err(AudioError::WindowsAudioUnavailable(
                "the RPC server is unavailable".to_owned(),
            ));
        }
        Ok(self.endpoints.clone())
    }

    fn defaults(&mut self, direction: DeviceDirection) -> Defaults {
        self.defaults.get(direction).clone()
    }

    fn set_default(
        &mut self,
        direction: DeviceDirection,
        id: &str,
        role: Role,
    ) -> Result<(), AudioError> {
        self.defaults
            .get_mut(direction)
            .set(role, Some(id.to_owned()));
        self.did(Did::SetDefault(direction, id.to_owned(), role));
        Ok(())
    }

    fn original_fxsound(&mut self) -> Option<String> {
        self.scans += 1;
        self.original.clone()
    }

    fn claimed(&mut self) -> PerDirection<Option<String>> {
        self.record.clone()
    }

    fn keep_claimed(&mut self, claimed: &PerDirection<Option<String>>) {
        self.record = claimed.clone();
        self.did(Did::Recorded(claimed.output.clone(), claimed.input.clone()));
    }

    fn users_cables(&mut self) -> PerDirection<Defaults> {
        self.users_cables.clone()
    }

    fn keep_users_cables(&mut self, cables: &PerDirection<Defaults>) {
        self.users_cables = cables.clone();
    }

    fn start_stream(
        &mut self,
        plan: StreamPlan,
        dsp: LaneDsp,
        system_mute: bool,
        gain: f32,
    ) -> Result<Box<dyn LaneStream>, StartFailure> {
        if self.refuse_streams {
            return Err((
                Box::new(Ended::with(
                    Some(dsp),
                    Outcome::Failed(AudioError::DeviceUnavailable),
                )),
                AudioError::DeviceUnavailable,
            ));
        }
        self.did(Did::Started(
            plan.direction,
            plan.capture.clone(),
            plan.render.clone(),
        ));
        let ended = Arc::new(AtomicBool::new(false));
        let outcome = Arc::new(std::sync::Mutex::new(Outcome::Stopped));
        *self.ended.get_mut(plan.direction) = Some((Arc::clone(&ended), Arc::clone(&outcome)));
        let format = StreamFormat {
            rate: 48_000,
            channels: 2,
            mask: 0x3,
        };
        Ok(Box::new(FakeStream {
            plan,
            dsp: Some(dsp),
            shared: Arc::new(StreamShared::new(system_mute, gain)),
            opened: Opened {
                format,
                packet: 480,
                period: 480,
                targets: Targets::new(512, 480, 480),
            },
            ended,
            outcome,
            log: Arc::clone(&self.log),
            stuck: Arc::clone(&self.stuck),
        }))
    }

    fn watch_volume(&mut self, _: &str, _: DeviceDirection) -> Option<Box<dyn VolumeWatch>> {
        self.gain
            .map(|gain| Box::new(Volume(gain)) as Box<dyn VolumeWatch>)
    }

    fn watch_sessions(&mut self, ids: &[String]) -> Vec<String> {
        self.watched = if self.sessions_refused {
            Vec::new()
        } else {
            ids.to_vec()
        };
        self.watched.clone()
    }

    fn watch_recorders(&mut self, id: Option<&str>) {
        self.recorder_watches += 1;
        self.recorders_watched = id.filter(|_| !self.sessions_refused).map(str::to_owned);
    }

    fn recorders(&mut self, id: &str) -> Recording {
        self.recorders_asked += 1;
        if self.recorders_unknown {
            return Recording::UNKNOWN;
        }
        Recording {
            active: Some(self.recorders.iter().any(|recorded| recorded == id)),
            heard: self.recorders_watched.as_deref() == Some(id),
        }
    }
}

/// An engine on a [`Fake`], the GUI's end of its notifications, and a clock the test moves.
struct Harness {
    engine: Engine<Fake>,
    ui: Receiver<AudioToUi>,
    now: Instant,
    log: Arc<std::sync::Mutex<Vec<Did>>>,
    _events: PerDirection<Receiver<DspEvent>>,
}

fn all_roles(id: &str) -> Defaults {
    let mut defaults = Defaults::default();
    for role in Role::ALL {
        defaults.set(role, Some(id.to_owned()));
    }
    defaults
}

fn harness_with(
    endpoints: Vec<Endpoint>,
    output_defaults: Defaults,
    input_defaults: Defaults,
    options: &StartOptions,
) -> Harness {
    harness_recorded(
        endpoints,
        output_defaults,
        input_defaults,
        options,
        PerDirection::default(),
    )
}

/// [`harness_with`], on a machine whose record of CrossFX's defaults (`record`) is `record`: what a
/// run before this one left.
fn harness_recorded(
    endpoints: Vec<Endpoint>,
    output_defaults: Defaults,
    input_defaults: Defaults,
    options: &StartOptions,
    record: PerDirection<Option<String>>,
) -> Harness {
    harness_kept(
        endpoints,
        output_defaults,
        input_defaults,
        options,
        record,
        PerDirection::default(),
    )
}

/// The next start after `previous`'s run: the same machine, with the defaults and CrossFX's record
/// as that run left them.
fn harness_after(previous: &mut Harness, options: &StartOptions) -> Harness {
    let fake = previous.fake();
    harness_kept(
        fake.endpoints.clone(),
        fake.defaults.output.clone(),
        fake.defaults.input.clone(),
        options,
        fake.record.clone(),
        fake.users_cables.clone(),
    )
}

/// [`harness_recorded`], with the record's word on the user's own cable defaults besides.
fn harness_kept(
    endpoints: Vec<Endpoint>,
    output_defaults: Defaults,
    input_defaults: Defaults,
    options: &StartOptions,
    record: PerDirection<Option<String>>,
    users_cables: PerDirection<Defaults>,
) -> Harness {
    let log = Arc::new(std::sync::Mutex::new(Vec::new()));
    let fake = Fake {
        endpoints,
        service_gone: false,
        defaults: PerDirection {
            output: output_defaults,
            input: input_defaults,
        },
        original: None,
        refuse_streams: false,
        gain: None,
        record,
        users_cables,
        log: Arc::clone(&log),
        ended: PerDirection::default(),
        stuck: Arc::default(),
        scans: 0,
        sessions_refused: false,
        watched: Vec::new(),
        recorders: Vec::new(),
        recorders_unknown: false,
        recorders_watched: None,
        recorder_watches: 0,
        recorders_asked: 0,
    };
    let (ui_tx, ui) = crossbeam_channel::unbounded();
    let (dsp, handover) = lanes_for_tests();
    let (lane_events, events) =
        PerDirection::from_fn(|_| crossbeam_channel::bounded(EVENT_QUEUE_LEN)).unzip();
    Harness {
        engine: Engine::new(fake, ui_tx, dsp, handover, lane_events, options),
        ui,
        now: Instant::now(),
        log,
        _events: events,
    }
}

/// The machine CrossFX is written on: the Realtek speakers and microphone array, and CrossFX's
/// driver — the original installed, not running — with every default on the speakers.
fn harness() -> Harness {
    harness_with(
        vec![speakers(), fxsound(), microphone()],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    )
}

fn headphones() -> Endpoint {
    Endpoint {
        form_factor: Some(3),
        ..endpoint(
            "{0.0.0.00000000}.{a8265ea8}",
            OUT,
            "Наушники (Realtek(R) Audio)",
            "Наушники",
            "Realtek(R) Audio",
        )
    }
}

impl Harness {
    fn start(&mut self) {
        self.engine.start(self.now);
    }

    fn send(&mut self, message: UiToAudio) {
        self.engine.control(message, self.now);
        self.engine.settle(self.now);
    }

    fn event(&mut self, event: Event) {
        self.engine.event(event, self.now);
        self.engine.settle(self.now);
    }

    /// An application starts recording from the recording endpoint `id` — a session of its own
    /// there, opened or started — which the engine hears of.
    fn record_from(&mut self, id: &str) {
        self.fake().recorders.push(id.to_owned());
        self.event(Event::Session(id.to_owned()));
    }

    /// Every application recording from `id` stops, which the engine hears of.
    fn stop_recording_from(&mut self, id: &str) {
        self.fake().recorders.retain(|recorded| recorded != id);
        self.event(Event::Session(id.to_owned()));
    }

    /// Several notifications taken in one drain of the channel, as the control thread takes a burst
    /// of them, and only then the settle.
    fn burst(&mut self, events: impl IntoIterator<Item = Event>) {
        for event in events {
            self.engine.event(event, self.now);
        }
        self.engine.settle(self.now);
    }

    fn tick(&mut self, by: Duration) {
        self.now += by;
        self.engine.tick(self.now);
    }

    fn messages(&self) -> Vec<AudioToUi> {
        self.ui.try_iter().collect()
    }

    /// What the engine asked of the system, the record's keeping left out.
    fn did(&self) -> Vec<Did> {
        self.did_all()
            .into_iter()
            .filter(|did| !matches!(did, Did::Recorded(..)))
            .collect()
    }

    /// What the engine asked of the system, the record's keeping included.
    fn did_all(&self) -> Vec<Did> {
        self.log.lock().map(|log| log.clone()).unwrap_or_default()
    }

    fn forget(&self) {
        if let Ok(mut log) = self.log.lock() {
            log.clear();
        }
        let _ = self.messages();
    }

    fn fake(&mut self) -> &mut Fake {
        &mut self.engine.system
    }

    fn defaults(&mut self, direction: DeviceDirection) -> Defaults {
        self.fake().defaults.get(direction).clone()
    }
}

/// What a claim of the lane of `direction` writes, and what giving it back writes: the roles the
/// lane takes (`Role::claimed_by`), each made `id`.
fn claimed(direction: DeviceDirection, id: &str) -> Vec<Did> {
    written(direction, id, Role::claimed_by(direction))
}

/// Every role made `id`: what giving back a default a run left in every role writes.
fn every_role(direction: DeviceDirection, id: &str) -> Vec<Did> {
    written(direction, id, &Role::ALL)
}

fn written(direction: DeviceDirection, id: &str, roles: &[Role]) -> Vec<Did> {
    roles
        .iter()
        .copied()
        .map(|role| Did::SetDefault(direction, id.to_owned(), role))
        .collect()
}

/// The output lane's virtual endpoint in the two playback roles, and calls on `calls`.
fn playback_on(id: &str, calls: &str) -> Defaults {
    let mut defaults = all_roles(id);
    defaults.set(Role::Communications, Some(calls.to_owned()));
    defaults
}

#[test]
fn the_output_lane_plays_the_virtual_device_to_the_speakers_and_takes_the_playback_roles_for_it() {
    let mut h = harness();
    h.start();
    let did = h.did();
    assert_eq!(
        did.first(),
        Some(&Did::Started(OUT, fxsound().id, speakers().id)),
        "the stream first, so that nothing played into the virtual device is lost"
    );
    assert_eq!(did[1..], claimed(OUT, &fxsound().id));
    assert_eq!(
        h.defaults(OUT),
        playback_on(&fxsound().id, &speakers().id),
        "calls stay where they were"
    );
    let messages = h.messages();
    assert!(messages.contains(&AudioToUi::RememberedDefault {
        direction: OUT,
        node_name: speakers().id,
    }));
    assert!(messages.contains(&AudioToUi::Attached {
        direction: OUT,
        node_name: Some(speakers().id),
    }));
    let devices = messages
        .iter()
        .find_map(|message| match message {
            AudioToUi::Devices(devices) => Some(devices.clone()),
            _ => None,
        })
        .expect("a device list");
    let names: Vec<&str> = devices.iter().map(|device| device.name.as_str()).collect();
    assert_eq!(
        names,
        [speakers().id.as_str(), microphone().id.as_str()],
        "the virtual device is never offered"
    );
}

#[test]
fn shutting_down_gives_the_default_back_before_the_stream_stops() {
    let mut h = harness();
    h.start();
    h.forget();
    h.engine.shutdown();
    let did = h.did();
    let mut expected = claimed(OUT, &speakers().id);
    expected.push(Did::Stopped(OUT, HAND_BACK_TAIL));
    assert_eq!(did, expected);
    assert_eq!(h.defaults(OUT), all_roles(&speakers().id));
}

#[test]
fn calls_stay_on_the_device_the_user_set_for_them_while_the_output_lane_plays() {
    // Music on the speakers, calls on the headphones — this machine's fifine headset, say.
    let defaults = playback_on(&speakers().id, &headphones().id);
    let mut h = harness_with(
        vec![speakers(), headphones(), fxsound(), microphone()],
        defaults.clone(),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.start();
    assert_eq!(
        h.defaults(OUT),
        playback_on(&fxsound().id, &headphones().id),
        "calls are not played through the lane's one device"
    );
    assert!(
        !h.did()
            .iter()
            .any(|did| matches!(did, Did::SetDefault(_, _, Role::Communications))),
        "{:?}",
        h.did()
    );
    // Every start, and every power off and on, leaves them there.
    h.send(UiToAudio::SetAsDefault {
        direction: OUT,
        want: false,
    });
    h.send(UiToAudio::SetAsDefault {
        direction: OUT,
        want: true,
    });
    assert_eq!(
        h.defaults(OUT),
        playback_on(&fxsound().id, &headphones().id)
    );
    h.engine.shutdown();
    assert_eq!(
        h.defaults(OUT),
        defaults,
        "music back to the speakers, calls never moved"
    );
    assert!(
        !h.did()
            .iter()
            .any(|did| matches!(did, Did::SetDefault(_, _, Role::Communications)))
    );
}

#[test]
fn the_power_off_gives_the_default_back_and_stops_and_the_power_on_takes_it_again() {
    let mut h = harness();
    h.start();
    h.forget();
    h.send(UiToAudio::SetAsDefault {
        direction: OUT,
        want: false,
    });
    let mut expected = claimed(OUT, &speakers().id);
    expected.push(Did::Stopped(OUT, HAND_BACK_TAIL));
    assert_eq!(h.did(), expected);
    h.tick(SUPERVISOR_PERIOD);
    let messages = h.messages();
    assert!(
        !messages
            .iter()
            .any(|message| matches!(message, AudioToUi::Attached { .. })),
        "still on the speakers, as far as the window is concerned: {messages:?}"
    );
    assert!(h.engine.lanes.output.stream.is_none());

    h.forget();
    h.send(UiToAudio::SetAsDefault {
        direction: OUT,
        want: true,
    });
    let did = h.did();
    assert_eq!(did[0], Did::Started(OUT, fxsound().id, speakers().id));
    assert_eq!(did[1..], claimed(OUT, &fxsound().id));
}

#[test]
fn starting_with_the_power_off_takes_nothing_and_plays_nothing() {
    let mut h = harness_with(
        vec![speakers(), fxsound(), microphone()],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions {
            want_default: false,
            ..StartOptions::default()
        },
    );
    h.start();
    assert_eq!(h.did(), []);
    assert!(h.messages().contains(&AudioToUi::Attached {
        direction: OUT,
        node_name: Some(speakers().id),
    }));
}

#[test]
fn with_no_virtual_device_the_output_lane_says_so_and_plays_nothing() {
    let mut h = harness_with(
        vec![speakers(), microphone()],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.start();
    assert_eq!(h.did(), []);
    let messages = h.messages();
    assert!(messages.contains(&AudioToUi::Error {
        direction: Some(OUT),
        message: AudioError::NoVirtualDevice.to_string(),
    }));
    assert!(!messages.iter().any(|message| matches!(
        message,
        AudioToUi::Attached {
            node_name: Some(_),
            ..
        }
    )));
    // Once, however often the rules run.
    h.send(UiToAudio::RescanDevices);
    h.tick(SUPERVISOR_PERIOD);
    assert!(
        !h.messages()
            .iter()
            .any(|message| matches!(message, AudioToUi::Error { .. }))
    );
}

#[test]
fn a_microphone_needs_a_cable_of_its_own_and_is_told_so_for_its_lane() {
    let mut h = harness();
    h.start();
    h.forget();
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    assert_eq!(h.did(), [], "the speakers' lane is left as it was");
    assert!(h.messages().contains(&AudioToUi::Error {
        direction: Some(IN),
        message: AudioError::NoVirtualInputDevice.to_string(),
    }));
    assert!(h.engine.lanes.output.stream.is_some());
}

#[test]
fn the_microphone_plays_into_a_cable_whose_recording_side_becomes_the_default() {
    let [cable_in, cable_out] = cable();
    let mut h = harness_with(
        vec![
            speakers(),
            fxsound(),
            microphone(),
            cable_in.clone(),
            cable_out.clone(),
        ],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.start();
    h.forget();
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    // The recording default on the cable at once, the microphone opened for nobody.
    assert_eq!(h.did(), claimed(IN, &cable_out.id));
    let messages = h.messages();
    assert!(messages.contains(&AudioToUi::RememberedDefault {
        direction: IN,
        node_name: microphone().id,
    }));
    assert!(messages.contains(&AudioToUi::Attached {
        direction: IN,
        node_name: Some(microphone().id),
    }));
    // Neither side of the cable is a device to pick.
    let devices = h.engine.devices();
    assert!(
        devices
            .iter()
            .all(|device| device.name != cable_in.id && device.name != cable_out.id)
    );
    // A call application starts recording from the default: the microphone plays into the cable.
    h.forget();
    h.record_from(&cable_out.id);
    assert_eq!(
        h.did(),
        [Did::Started(IN, microphone().id, cable_in.id.clone())]
    );

    // Detached, it gives the recording default back to the microphone and says so.
    h.forget();
    h.send(UiToAudio::DetachLane(IN));
    let mut expected = claimed(IN, &microphone().id);
    expected.push(Did::Stopped(IN, HAND_BACK_TAIL));
    assert_eq!(h.did(), expected);
    assert!(h.messages().contains(&AudioToUi::Attached {
        direction: IN,
        node_name: None,
    }));
}

/// The machine the input lane's tests run on: FxSound's driver for the speakers, VB-Cable for the
/// microphone, which is picked — and no application recording from the cable yet.
fn microphone_on_its_cable() -> Harness {
    let [cable_in, cable_out] = cable();
    let mut h = harness_with(
        vec![speakers(), fxsound(), microphone(), cable_in, cable_out],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.start();
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    h
}

/// Whether the input lane has its microphone open.
fn microphone_open(h: &Harness) -> bool {
    h.engine.lanes.input.stream.is_some()
}

#[test]
fn the_microphone_is_open_only_while_something_records_from_the_cable() {
    let [cable_in, cable_out] = cable();
    let mut h = microphone_on_its_cable();
    assert!(
        !microphone_open(&h),
        "nobody records: the microphone stays closed"
    );
    assert_eq!(
        h.engine.lanes.input.attachment(),
        Some(microphone().id.as_str()),
        "attached all the same"
    );
    assert_eq!(
        h.defaults(IN),
        all_roles(&cable_out.id),
        "and the default is on the cable, where a recorder will look"
    );
    assert_eq!(h.fake().recorders_watched, Some(cable_out.id.clone()));
    h.tick(SUPERVISOR_PERIOD);
    assert!(!microphone_open(&h));
    // A call application starts recording — a session opened, or one it had started again — and
    // is heard at once.
    h.forget();
    h.record_from(&cable_out.id);
    assert_eq!(
        h.did(),
        [Did::Started(IN, microphone().id, cable_in.id.clone())]
    );
    // It stops. The microphone stays open a moment, for a recorder that starts again, and then
    // closes; the default stays on the cable and the lane attached.
    h.forget();
    h.stop_recording_from(&cable_out.id);
    h.tick(SUPERVISOR_PERIOD);
    assert_eq!(h.did(), []);
    for _ in 0..(RECORDER_LINGER.as_millis() / SUPERVISOR_PERIOD.as_millis()) {
        h.tick(SUPERVISOR_PERIOD);
    }
    assert_eq!(h.did(), [Did::Stopped(IN, Duration::ZERO)]);
    assert!(!microphone_open(&h));
    assert_eq!(h.defaults(IN), all_roles(&cable_out.id));
    assert_eq!(
        h.engine.lanes.input.attachment(),
        Some(microphone().id.as_str())
    );
    // A recorder that starts within the moment finds the microphone still open.
    h.record_from(&cable_out.id);
    h.stop_recording_from(&cable_out.id);
    h.forget();
    h.tick(SUPERVISOR_PERIOD);
    h.record_from(&cable_out.id);
    for _ in 0..(RECORDER_LINGER.as_millis() / SUPERVISOR_PERIOD.as_millis()) {
        h.tick(SUPERVISOR_PERIOD);
    }
    assert_eq!(h.did(), [], "never closed in between");
    assert!(microphone_open(&h));
}

#[test]
fn who_records_is_heard_not_asked_on_every_tick() {
    let [_, cable_out] = cable();
    let mut h = microphone_on_its_cable();
    // A minute in the tray with nobody recording: the cable is asked on the slow check alone.
    h.fake().recorders_asked = 0;
    for _ in 0..300 {
        h.tick(SUPERVISOR_PERIOD);
    }
    let asked = h.fake().recorders_asked;
    let checks = (Duration::from_secs(60).as_millis() / RECORDERS_CHECK.as_millis()) as usize;
    assert!(
        (checks - 1..=checks + 1).contains(&asked),
        "{asked} asks in a minute"
    );
    assert!(!microphone_open(&h));
    // A recorder whose start went unheard is found by the slow check all the same.
    h.fake().recorders.push(cable_out.id.clone());
    for _ in 0..(RECORDERS_CHECK.as_millis() / SUPERVISOR_PERIOD.as_millis()) {
        h.tick(SUPERVISOR_PERIOD);
    }
    assert!(microphone_open(&h));
}

#[test]
fn a_recorder_already_recording_from_the_cable_has_the_microphone_open_at_once() {
    let [cable_in, cable_out] = cable();
    let mut h = harness_with(
        vec![
            speakers(),
            fxsound(),
            microphone(),
            cable_in.clone(),
            cable_out.clone(),
        ],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.start();
    // A program set to record from "CABLE Output" itself records already: no session of its is
    // opened or started once the lane is there, and none needs to be.
    h.fake().recorders.push(cable_out.id.clone());
    h.forget();
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    assert_eq!(
        h.did().first(),
        Some(&Did::Started(IN, microphone().id, cable_in.id.clone())),
        "{:?}",
        h.did()
    );
}

#[test]
fn where_who_records_is_not_heard_it_is_asked_on_every_tick() {
    let [cable_in, cable_out] = cable();
    let mut h = harness_with(
        vec![
            speakers(),
            fxsound(),
            microphone(),
            cable_in.clone(),
            cable_out.clone(),
        ],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.fake().sessions_refused = true;
    h.start();
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    assert!(!microphone_open(&h));
    // A recorder starts, and nothing tells of it: the next tick finds it.
    h.forget();
    h.fake().recorders.push(cable_out.id.clone());
    h.tick(SUPERVISOR_PERIOD);
    assert_eq!(
        h.did(),
        [Did::Started(IN, microphone().id, cable_in.id.clone())]
    );
}

#[test]
fn the_app_holds_the_microphone_open_with_nobody_recording_and_the_unknown_counts_as_someone() {
    let mut h = microphone_on_its_cable();
    // The calibration starts.
    h.send(UiToAudio::KeepInputAwake(true));
    assert!(microphone_open(&h));
    // It ends: closed once the moment it is kept open for is up.
    h.send(UiToAudio::KeepInputAwake(false));
    assert!(microphone_open(&h));
    for _ in 0..=(RECORDER_LINGER.as_millis() / SUPERVISOR_PERIOD.as_millis()) {
        h.tick(SUPERVISOR_PERIOD);
    }
    assert!(!microphone_open(&h));
    // The sessions can no longer be asked — the audio service gone, while no stream runs to
    // notice: the microphone is open for nothing rather than missing from a call, and the
    // endpoints are read again, the watch made anew with them.
    h.fake().recorders_unknown = true;
    let watches = h.fake().recorder_watches;
    for _ in 0..=(RECORDERS_CHECK.as_millis() / SUPERVISOR_PERIOD.as_millis()) {
        h.tick(SUPERVISOR_PERIOD);
    }
    assert!(microphone_open(&h));
    assert!(h.fake().recorder_watches > watches);
    // Back: nobody records, and the microphone closes once its moment is up.
    h.fake().recorders_unknown = false;
    for _ in 0..=((RECORDERS_CHECK + RECORDER_LINGER).as_millis() / SUPERVISOR_PERIOD.as_millis()) {
        h.tick(SUPERVISOR_PERIOD);
    }
    assert!(!microphone_open(&h));
}

#[test]
fn a_microphone_waiting_for_a_recorder_hands_its_default_back_before_the_speakers_take_its_cable() {
    // VB-Cable alone, the speakers' lane off, the microphone on the cable with nobody recording.
    let [cable_in, cable_out] = cable();
    let mut h = harness_with(
        vec![
            speakers(),
            microphone(),
            cable_in.clone(),
            cable_out.clone(),
        ],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions {
            output_enabled: false,
            ..StartOptions::default()
        },
    );
    h.start();
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    assert!(!microphone_open(&h));
    assert_eq!(h.defaults(IN), all_roles(&cable_out.id));
    h.forget();
    h.send(UiToAudio::SelectDevice {
        node_name: speakers().id,
        direction: OUT,
    });
    let did = h.did();
    let handed_back = did
        .iter()
        .position(|did| *did == Did::SetDefault(IN, microphone().id, Role::Console))
        .expect("the recording default goes back to the microphone");
    let started = did
        .iter()
        .position(|did| matches!(did, Did::Started(OUT, capture, _) if *capture == cable_in.id))
        .expect("the speakers' lane starts on the cable");
    assert!(handed_back < started, "{did:?}");
    assert_eq!(h.fake().recorders_watched, None, "nothing left to watch");
}

#[test]
fn switching_the_speakers_lane_off_and_on_leaves_the_microphone_on_its_cable() {
    // No FxSound driver here: the speakers stand on VB-Cable, and the microphone on Cable A.
    let [c_in, c_out] = cable();
    let [a_in, a_out] = cable_a();
    let mut h = harness_with(
        vec![
            speakers(),
            microphone(),
            c_in.clone(),
            c_out,
            a_in.clone(),
            a_out.clone(),
        ],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.start();
    assert_eq!(
        h.engine
            .lanes
            .output
            .stream
            .as_ref()
            .map(|stream| stream.plan().capture.clone()),
        Some(c_in.id.clone())
    );
    // A call application records from the microphone's cable meanwhile.
    h.fake().recorders.push(a_out.id.clone());
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    assert_eq!(
        h.engine
            .lanes
            .input
            .stream
            .as_ref()
            .map(|stream| stream.plan().render.clone()),
        Some(a_in.id.clone())
    );
    let microphone_moved = |did: &[Did]| {
        did.iter().any(|did| {
            matches!(
                did,
                Did::Stopped(IN, _) | Did::Started(IN, ..) | Did::SetDefault(IN, ..)
            )
        })
    };
    // VB-Cable is free once the speakers' lane is off, and sorts first: no reason to move every
    // recording application off Cable A.
    h.forget();
    h.send(UiToAudio::DetachLane(OUT));
    h.tick(SUPERVISOR_PERIOD);
    assert!(!microphone_moved(&h.did()), "{:?}", h.did());
    // Nor is it moved when the speakers come back and take VB-Cable again.
    h.forget();
    h.send(UiToAudio::SelectDevice {
        node_name: speakers().id,
        direction: OUT,
    });
    h.tick(SUPERVISOR_PERIOD);
    assert!(!microphone_moved(&h.did()), "{:?}", h.did());
    assert_eq!(h.defaults(IN), all_roles(&a_out.id));
}

/// Where in `did` the input lane stopped, and where the output lane started capturing `cable`.
fn microphone_stopped_and_speakers_started(did: &[Did], cable: &str) -> (usize, usize) {
    let stopped = did
        .iter()
        .position(|did| matches!(did, Did::Stopped(IN, _)))
        .unwrap_or_else(|| panic!("the microphone's stream stops: {did:?}"));
    let started = did
        .iter()
        .position(|did| matches!(did, Did::Started(OUT, capture, _) if capture == cable))
        .unwrap_or_else(|| panic!("the speakers' lane starts on the cable: {did:?}"));
    (stopped, started)
}

#[test]
fn the_speakers_lane_switched_on_onto_the_microphone_s_cable_takes_it_once_the_microphone_is_off() {
    // VB-Cable alone, and the speakers' lane off: the microphone plays into the cable.
    let [cable_in, cable_out] = cable();
    let mut h = harness_with(
        vec![
            speakers(),
            microphone(),
            cable_in.clone(),
            cable_out.clone(),
        ],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions {
            output_enabled: false,
            ..StartOptions::default()
        },
    );
    h.start();
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    // A call is being recorded from it.
    h.record_from(&cable_out.id);
    assert_eq!(h.defaults(IN), all_roles(&cable_out.id));
    h.forget();
    // The user switches the speakers' lane on by picking the speakers.
    h.send(UiToAudio::SelectDevice {
        node_name: speakers().id,
        direction: OUT,
    });
    let did = h.did();
    let (stopped, started) = microphone_stopped_and_speakers_started(&did, &cable_in.id);
    assert!(
        stopped < started,
        "nothing captured from the cable while the microphone still plays into it: {did:?}"
    );
    let handed_back = did
        .iter()
        .position(|did| *did == Did::SetDefault(IN, microphone().id, Role::Console))
        .expect("the recording default goes back to the microphone");
    assert!(handed_back < started, "{did:?}");
    assert_eq!(
        did[stopped],
        Did::Stopped(IN, HAND_BACK_TAIL),
        "its tail plays into the cable while the recorders move, before anything captures it"
    );
    assert!(h.messages().contains(&AudioToUi::Error {
        direction: Some(IN),
        message: AudioError::NoVirtualInputDevice.to_string(),
    }));
}

#[test]
fn a_device_change_that_moves_the_speakers_onto_the_microphone_s_cable_stops_the_microphone_first()
{
    // FxSound's driver for the speakers, VB-Cable for the microphone — until the driver goes.
    let [cable_in, cable_out] = cable();
    let mut h = harness_with(
        vec![
            speakers(),
            fxsound(),
            microphone(),
            cable_in.clone(),
            cable_out.clone(),
        ],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.start();
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    h.record_from(&cable_out.id);
    h.forget();
    h.fake()
        .endpoints
        .retain(|endpoint| endpoint.id != fxsound().id);
    h.event(Event::Devices);
    let did = h.did();
    let (stopped, started) = microphone_stopped_and_speakers_started(&did, &cable_in.id);
    assert!(stopped < started, "{did:?}");
}

#[test]
fn while_the_original_fxsound_runs_the_output_lane_stands_aside_and_says_why() {
    let mut h = harness();
    h.fake().original = Some(r"C:\Program Files\FxSound LLC\FxSound\FxSound.exe".to_owned());
    h.start();
    assert_eq!(h.did(), [], "neither a default nor a stream");
    let messages = h.messages();
    assert!(messages.contains(&AudioToUi::Warning {
        direction: Some(OUT),
        message: crossfx_core::i18n::tr(ORIGINAL_FXSOUND_RUNNING),
    }));
    assert!(messages.contains(&AudioToUi::Attached {
        direction: OUT,
        node_name: Some(speakers().id),
    }));

    // It quits: the lane takes over.
    h.fake().original = None;
    h.tick(ORIGINAL_EVERY);
    let did = h.did();
    assert_eq!(
        did.first(),
        Some(&Did::Started(OUT, fxsound().id, speakers().id))
    );
    assert_eq!(did[1..], claimed(OUT, &fxsound().id));
}

#[test]
fn the_original_starting_while_crossfx_holds_the_default_is_left_the_default_without_a_fight() {
    // Its start shows at once, as its capture of its driver: a session on "FxSound Speakers". And
    // with no watch on the driver's sessions, on the next short beat of the process list.
    for watched in [true, false] {
        let mut h = harness();
        h.fake().sessions_refused = !watched;
        h.start();
        h.forget();
        h.fake().original = Some("FxSound.exe (process 12364)".to_owned());
        if watched {
            h.event(Event::Session(fxsound().id));
        } else {
            h.tick(ORIGINAL_EVERY);
        }
        assert_eq!(
            h.did(),
            [Did::Stopped(OUT, Duration::ZERO)],
            "stopped, and no default written: it is the original's now"
        );
        assert!(h.messages().contains(&AudioToUi::Warning {
            direction: Some(OUT),
            message: crossfx_core::i18n::tr(ORIGINAL_FXSOUND_RUNNING),
        }));
        // And nothing is handed back at the exit either.
        h.forget();
        h.engine.shutdown();
        assert_eq!(h.did(), []);
    }
}

#[test]
fn the_original_fxsound_is_never_looked_for_on_a_machine_without_its_driver() {
    let [cable_in, cable_out] = cable();
    let mut h = harness_with(
        vec![speakers(), microphone(), cable_in, cable_out],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.start();
    for _ in 0..300 {
        h.tick(SUPERVISOR_PERIOD);
    }
    assert_eq!(h.fake().scans, 0, "it cannot run without its driver");
}

#[test]
fn the_process_list_is_read_rarely_while_the_original_s_start_would_show_by_itself() {
    let mut h = harness();
    h.start();
    assert_eq!(h.fake().scans, 1, "looked for once its driver is listed");
    assert_eq!(
        h.fake().watched,
        [fxsound().id],
        "and its driver's sessions watched"
    );
    // A minute in the tray, the lane playing: the fallback's two reads, no more.
    for _ in 0..300 {
        h.tick(SUPERVISOR_PERIOD);
    }
    assert_eq!(
        h.fake().scans,
        3,
        "the start's look and two of the fallback's"
    );
    // An application starts playing into the driver: a session, and a look.
    h.event(Event::Session(fxsound().id));
    assert_eq!(h.fake().scans, 4);
    // A session on another endpoint is no news of the original's.
    h.event(Event::Session(speakers().id));
    assert_eq!(h.fake().scans, 4);

    // While it runs, it is looked at on the short beat — its process watched, at no cost — and
    // its quitting is seen there.
    h.tick(ORIGINAL_EVERY);
    h.fake().original = Some("FxSound.exe (process 12364)".to_owned());
    h.event(Event::Session(fxsound().id));
    assert!(h.engine.original.is_some());
    let before = h.fake().scans;
    h.tick(ORIGINAL_EVERY);
    assert_eq!(h.fake().scans, before + 1);
    h.fake().original = None;
    h.tick(ORIGINAL_EVERY);
    assert!(h.engine.original.is_none());
}

#[test]
fn sessions_opened_on_the_driver_one_after_another_have_the_process_list_read_once_a_beat() {
    let mut h = harness();
    h.start();
    let scans = h.fake().scans;
    // A program that opens a session of its own for every sound it plays: fifty in a second.
    for _ in 0..50 {
        h.now += Duration::from_millis(20);
        h.event(Event::Session(fxsound().id));
    }
    assert_eq!(h.fake().scans, scans + 1, "read at the first, not at each");
    // The original starting among them is still found on the beat after that first look.
    h.fake().original = Some("FxSound.exe (process 12364)".to_owned());
    h.event(Event::Session(fxsound().id));
    assert!(h.engine.original.is_none());
    h.tick(ORIGINAL_EVERY);
    assert!(h.engine.original.is_some());
    assert_eq!(h.fake().scans, scans + 2);
}

#[test]
fn sessions_out_of_step_with_the_beat_have_the_process_list_read_once_a_beat_too() {
    // A session every 330 ms, or every 130 ms, with the supervisor's ticks between them: neither
    // pace divides the beat, so a session comes just after the look a waiting one had on it.
    for period in [Duration::from_millis(330), Duration::from_millis(130)] {
        let mut h = harness();
        h.start();
        let start = h.now;
        let end = start + Duration::from_secs(20);
        let mut scans = h.fake().scans;
        let mut reads = Vec::new();
        let mut next_tick = start + SUPERVISOR_PERIOD;
        let mut next_session = start + period;
        while next_tick.min(next_session) <= end {
            if next_session < next_tick {
                h.now = next_session;
                h.event(Event::Session(fxsound().id));
                next_session += period;
            } else {
                let by = next_tick - h.now;
                h.tick(by);
                next_tick += SUPERVISOR_PERIOD;
            }
            if h.fake().scans > scans {
                scans = h.fake().scans;
                reads.push(h.now - start);
            }
        }
        let gaps: Vec<Duration> = reads.windows(2).map(|pair| pair[1] - pair[0]).collect();
        assert!(
            gaps.iter().all(|&gap| gap >= ORIGINAL_EVERY),
            "{period:?}: no two reads within a beat, read at {reads:?}"
        );
        // And on every beat: the original starting among them is found within one.
        assert!(
            gaps.iter()
                .all(|&gap| gap <= ORIGINAL_EVERY + SUPERVISOR_PERIOD),
            "{period:?}: read at {reads:?}"
        );
        assert!(reads.len() >= 9, "{period:?}: read at {reads:?}");
    }
}

#[test]
fn a_default_moved_onto_the_driver_by_another_program_has_the_original_looked_for_at_once() {
    // The power is off: CrossFX holds no default, and its process list is read on the fallback.
    let mut h = harness_with(
        vec![speakers(), headphones(), fxsound(), microphone()],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions {
            want_default: false,
            ..StartOptions::default()
        },
    );
    h.start();
    let scans = h.fake().scans;
    // The original starts and claims its driver.
    h.fake().original = Some("FxSound.exe (process 12364)".to_owned());
    h.fake().defaults.output = playback_on(&fxsound().id, &speakers().id);
    h.burst(playback_default_changed(&fxsound().id));
    assert_eq!(h.fake().scans, scans + 1, "looked for at the first move");
    assert!(h.engine.original.is_some());
    // It quits, handing the default back to the headphones it played to: not the user's pick.
    h.fake().defaults.output = playback_on(&headphones().id, &speakers().id);
    h.burst(playback_default_changed(&headphones().id));
    assert!(
        !h.messages()
            .iter()
            .any(|message| matches!(message, AudioToUi::DesktopPick { .. })),
        "the original's hand-back is its own"
    );
}

#[test]
fn an_original_taken_to_run_is_let_go_of_with_its_driver() {
    let mut h = harness();
    h.fake().original = Some("FxSound.exe (process 12364)".to_owned());
    h.start();
    assert!(h.engine.original.is_some());
    // Its driver is uninstalled: without it the original cannot run.
    h.fake()
        .endpoints
        .retain(|endpoint| endpoint.id != fxsound().id);
    h.event(Event::Devices);
    assert!(h.engine.original.is_none());
    assert!(
        h.fake().watched.is_empty(),
        "nothing of the driver's left to watch"
    );
}

#[test]
fn a_device_the_desktop_picks_is_the_users_pick_and_the_default_is_taken_back() {
    let mut h = harness_with(
        vec![speakers(), headphones(), fxsound(), microphone()],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.start();
    // CrossFX's own writes come back first, and are no pick.
    for &role in Role::claimed_by(OUT) {
        h.event(Event::DefaultChanged {
            direction: OUT,
            role,
            id: Some(fxsound().id),
        });
    }
    h.forget();
    // The sound settings pick the headphones.
    for role in [Role::Console, Role::Multimedia] {
        h.fake().defaults.output.set(role, Some(headphones().id));
        h.event(Event::DefaultChanged {
            direction: OUT,
            role,
            id: Some(headphones().id),
        });
    }
    let messages = h.messages();
    assert_eq!(
        messages
            .iter()
            .filter(|message| matches!(message, AudioToUi::DesktopPick { .. }))
            .collect::<Vec<_>>(),
        [&AudioToUi::DesktopPick {
            direction: OUT,
            node_name: headphones().id,
        }]
    );
    let did = h.did();
    assert_eq!(
        did[..2],
        [
            Did::Stopped(OUT, Duration::ZERO),
            Did::Started(OUT, fxsound().id, headphones().id)
        ],
        "moved, the default kept on the virtual device in between: {did:?}"
    );
    assert_eq!(
        did[2..],
        [
            Did::SetDefault(OUT, fxsound().id, Role::Console),
            Did::SetDefault(OUT, fxsound().id, Role::Multimedia),
        ],
        "and the two roles the desktop moved are taken back"
    );
    // Given back at the end to the device the user picked last.
    h.forget();
    h.engine.shutdown();
    assert_eq!(
        h.defaults(OUT).get(Role::Console),
        Some(headphones().id.as_str())
    );
    assert_eq!(
        h.defaults(OUT).get(Role::Multimedia),
        Some(headphones().id.as_str())
    );
}

#[test]
fn with_a_ranking_a_desktop_pick_moves_nothing() {
    let mut h = harness_with(
        vec![speakers(), headphones(), fxsound(), microphone()],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions {
            output_priority: DevicePriority {
                names: vec![speakers().id],
                new_devices_first: false,
            },
            ..StartOptions::default()
        },
    );
    h.start();
    h.forget();
    h.fake()
        .defaults
        .output
        .set(Role::Multimedia, Some(headphones().id));
    h.event(Event::DefaultChanged {
        direction: OUT,
        role: Role::Multimedia,
        id: Some(headphones().id),
    });
    assert_eq!(h.did(), []);
    assert!(
        !h.messages()
            .iter()
            .any(|message| matches!(message, AudioToUi::DesktopPick { .. }))
    );
    // Nor is the default taken back however often the rules run after it: the ranking decides
    // the lane's device, and the user decided the default.
    h.event(Event::Devices);
    h.tick(SUPERVISOR_PERIOD);
    assert_eq!(h.did(), []);
    h.engine.shutdown();
    assert_eq!(
        h.defaults(OUT).get(Role::Multimedia),
        Some(headphones().id.as_str()),
        "and it is not written over at the exit"
    );
}

/// A headset's microphone, for calls: what a user may record calls from instead of CrossFX.
fn headset_microphone() -> Endpoint {
    Endpoint {
        enumerator: "USB".to_owned(),
        ..endpoint(
            "{0.0.1.00000000}.{fifine-mic}",
            IN,
            "Микрофон (3- fifine Microphone)",
            "Микрофон",
            "3- fifine Microphone",
        )
    }
}

/// A USB headset's headphones, plugged in during a test.
fn usb_headset() -> Endpoint {
    Endpoint {
        enumerator: "USB".to_owned(),
        form_factor: Some(3),
        ..endpoint(
            "{0.0.0.00000000}.{usb-headset}",
            OUT,
            "Наушники (USB Headset)",
            "Наушники",
            "USB Headset",
        )
    }
}

/// Windows making `id` the default for the playback roles the output lane holds.
fn playback_default_changed(id: &str) -> impl Iterator<Item = Event> + '_ {
    Role::claimed_by(OUT)
        .iter()
        .map(move |&role| Event::DefaultChanged {
            direction: OUT,
            role,
            id: Some(id.to_owned()),
        })
}

/// What the engine did and said when `usb_headset()` was plugged in and Windows made it the
/// default: the notifications in one burst, or the device's apart from its default's.
fn plug_in_the_headset(one_burst: bool) -> (Vec<Did>, Vec<AudioToUi>, Defaults) {
    let mut h = harness();
    h.start();
    h.forget();
    h.fake().endpoints.push(usb_headset());
    for &role in Role::claimed_by(OUT) {
        h.fake().defaults.output.set(role, Some(usb_headset().id));
    }
    let headset = usb_headset().id;
    if one_burst {
        h.burst(std::iter::once(Event::Devices).chain(playback_default_changed(&headset)));
    } else {
        h.event(Event::Devices);
        h.burst(playback_default_changed(&headset));
    }
    h.tick(SUPERVISOR_PERIOD);
    (h.did(), h.messages(), h.defaults(OUT))
}

#[test]
fn a_device_plugged_in_and_made_the_default_in_one_burst_is_followed_as_when_the_two_come_apart() {
    let (apart, _, apart_defaults) = plug_in_the_headset(false);
    let (burst, messages, defaults) = plug_in_the_headset(true);
    assert_eq!(burst, apart, "the same physical event, the same outcome");
    assert_eq!(defaults, apart_defaults);
    assert!(
        burst.contains(&Did::Started(OUT, fxsound().id, usb_headset().id)),
        "{burst:?}"
    );
    assert!(messages.contains(&AudioToUi::DesktopPick {
        direction: OUT,
        node_name: usb_headset().id,
    }));
    assert_eq!(
        defaults,
        playback_on(&fxsound().id, &speakers().id),
        "the playback roles are CrossFX's again: the headset is heard through it"
    );
}

#[test]
fn a_default_on_an_endpoint_not_readable_yet_waits_for_it_and_is_never_left_with_the_user() {
    let mut h = harness();
    h.start();
    h.forget();
    // Windows makes the headset the default before its property store can be read: the list
    // the engine reads does not have it yet.
    for &role in Role::claimed_by(OUT) {
        h.fake().defaults.output.set(role, Some(usb_headset().id));
    }
    let headset = usb_headset().id;
    h.burst(playback_default_changed(&headset));
    assert_eq!(h.did(), [], "neither followed nor taken back");
    let claim = h
        .engine
        .lanes
        .output
        .claim
        .as_ref()
        .expect("the claim stands");
    assert!(!claim.has_yielded(Role::Console) && !claim.has_yielded(Role::Multimedia));
    // A moment later it can be read: the next read finds it, and the pick is followed.
    h.fake().endpoints.push(usb_headset());
    h.tick(SUPERVISOR_PERIOD);
    assert!(h.messages().contains(&AudioToUi::DesktopPick {
        direction: OUT,
        node_name: usb_headset().id,
    }));
    assert_eq!(h.defaults(OUT), playback_on(&fxsound().id, &speakers().id));

    // One that never becomes readable is let go of once its wait is up: never yielded, and the
    // endpoints are no longer read on every tick for it.
    let mut h = harness();
    h.start();
    let ghost = "{0.0.0.00000000}.{never-readable}";
    for &role in Role::claimed_by(OUT) {
        h.fake().defaults.output.set(role, Some(ghost.to_owned()));
    }
    h.burst(playback_default_changed(ghost));
    for _ in 0..=(PICK_WAIT.as_millis() / SUPERVISOR_PERIOD.as_millis()) {
        h.tick(SUPERVISOR_PERIOD);
    }
    assert!(h.engine.pending.output.is_empty());
    let claim = h
        .engine
        .lanes
        .output
        .claim
        .as_ref()
        .expect("the claim stands");
    assert!(!claim.has_yielded(Role::Console) && !claim.has_yielded(Role::Multimedia));
}

#[test]
fn windows_moving_a_default_off_a_device_that_went_is_no_pick_and_the_saved_device_stays() {
    let mut h = harness_with(
        vec![speakers(), headphones(), fxsound(), microphone()],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.start();
    // The user listens on the headphones, picked in CrossFX.
    h.send(UiToAudio::SelectDevice {
        node_name: headphones().id,
        direction: OUT,
    });
    h.forget();
    // "FxSound Speakers" is disabled, and Windows moves the playback roles to the speakers.
    h.fake()
        .endpoints
        .retain(|endpoint| endpoint.id != fxsound().id);
    for &role in Role::claimed_by(OUT) {
        h.fake().defaults.output.set(role, Some(speakers().id));
    }
    let to = speakers().id;
    h.burst(std::iter::once(Event::Devices).chain(playback_default_changed(&to)));
    assert!(
        !h.messages()
            .iter()
            .any(|message| matches!(message, AudioToUi::DesktopPick { .. })),
        "Windows' fallback is not the user's pick"
    );
    assert_eq!(h.engine.memory.output.user_selected, headphones().id);
    // It comes back: the lane plays where the user picked, not where Windows fell back to.
    h.fake().endpoints.push(fxsound());
    h.forget();
    h.event(Event::Devices);
    assert_eq!(
        h.did().first(),
        Some(&Did::Started(OUT, fxsound().id, headphones().id)),
        "{:?}",
        h.did()
    );
}

#[test]
fn a_fallback_onto_fxsound_s_driver_with_the_power_off_is_given_back_rather_than_left_silent() {
    // The power is off: the lane plays nothing, the default is on the headphones.
    let mut h = harness_with(
        vec![speakers(), headphones(), fxsound(), microphone()],
        all_roles(&headphones().id),
        all_roles(&microphone().id),
        &StartOptions {
            want_default: false,
            ..StartOptions::default()
        },
    );
    h.start();
    h.forget();
    // They are unplugged, and Windows falls back to the endpoint last made the default before
    // them: FxSound's driver, which nothing captures now.
    h.fake()
        .endpoints
        .retain(|endpoint| endpoint.id != headphones().id);
    h.fake().defaults.output = all_roles(&fxsound().id);
    let to = fxsound().id;
    h.burst(
        std::iter::once(Event::Devices).chain(Role::ALL.into_iter().map(|role| {
            Event::DefaultChanged {
                direction: OUT,
                role,
                id: Some(to.clone()),
            }
        })),
    );
    h.tick(SUPERVISOR_PERIOD);
    assert_eq!(
        h.defaults(OUT),
        all_roles(&speakers().id),
        "given back to a device that plays"
    );
    assert!(
        !h.messages()
            .iter()
            .any(|message| matches!(message, AudioToUi::DesktopPick { .. }))
    );
}

#[test]
fn a_virtual_device_renamed_in_the_sound_settings_is_still_the_device_the_lanes_stand_on() {
    // A ranking decides the speakers' device, so the user's own pick of a default is theirs: the
    // claim on FxSound's driver yields it, and must stand.
    let [cable_in, cable_out] = cable();
    let mut h = harness_with(
        vec![
            speakers(),
            headphones(),
            fxsound(),
            microphone(),
            cable_in.clone(),
            cable_out,
        ],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions {
            output_priority: DevicePriority {
                names: vec![speakers().id],
                new_devices_first: false,
            },
            ..StartOptions::default()
        },
    );
    h.start();
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    h.fake()
        .defaults
        .output
        .set(Role::Multimedia, Some(headphones().id));
    h.event(Event::DefaultChanged {
        direction: OUT,
        role: Role::Multimedia,
        id: Some(headphones().id),
    });
    h.forget();
    // "FxSound Speakers" renamed "Music", and "CABLE Input" "Voice": the same endpoints.
    for endpoint in &mut h.fake().endpoints {
        if endpoint.id == fxsound().id {
            endpoint.friendly_name = "Music (FxSound Audio Enhancer)".to_owned();
            endpoint.description = "Music".to_owned();
        } else if endpoint.id == cable_in.id {
            endpoint.friendly_name = "Voice (VB-Audio Virtual Cable)".to_owned();
            endpoint.description = "Voice".to_owned();
        }
    }
    // Nothing reads the rename until the endpoints are read again for something else.
    h.event(Event::Devices);
    h.tick(SUPERVISOR_PERIOD);
    assert_eq!(
        h.did(),
        [],
        "neither lane handed its default back, stopped or claimed again"
    );
    assert!(
        !h.messages()
            .iter()
            .any(|message| matches!(message, AudioToUi::RememberedDefault { .. }))
    );
    assert_eq!(
        h.defaults(OUT).get(Role::Multimedia),
        Some(headphones().id.as_str()),
        "the role the user took stays theirs"
    );
    assert_eq!(
        h.engine
            .lanes
            .output
            .virtual_device
            .as_ref()
            .map(|device| device.name.as_str()),
        Some("Music (FxSound Audio Enhancer)"),
        "the new name is the one the log uses"
    );
}

#[test]
fn calls_the_user_moves_while_crossfx_holds_them_stay_where_they_were_put() {
    let [cable_in, cable_out] = cable();
    let mut h = harness_with(
        vec![
            speakers(),
            fxsound(),
            microphone(),
            headset_microphone(),
            cable_in,
            cable_out.clone(),
        ],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.start();
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    for &role in Role::claimed_by(IN) {
        h.event(Event::DefaultChanged {
            direction: IN,
            role,
            id: Some(cable_out.id.clone()),
        });
    }
    h.forget();
    // A call application's microphone, picked in the sound settings.
    h.fake()
        .defaults
        .input
        .set(Role::Communications, Some(headset_microphone().id));
    h.event(Event::DefaultChanged {
        direction: IN,
        role: Role::Communications,
        id: Some(headset_microphone().id),
    });
    // A device comes and goes, and the rules run again; so does a supervisor tick.
    h.event(Event::Devices);
    h.tick(SUPERVISOR_PERIOD);
    assert_eq!(
        h.did(),
        [],
        "calls are not taken back, and the microphone never moved"
    );
    assert!(
        !h.messages()
            .iter()
            .any(|message| matches!(message, AudioToUi::DesktopPick { .. })),
        "calls are not where the lane records"
    );
    h.engine.shutdown();
    assert_eq!(
        h.defaults(IN).get(Role::Communications),
        Some(headset_microphone().id.as_str())
    );
    assert_eq!(
        h.defaults(IN).get(Role::Multimedia),
        Some(microphone().id.as_str())
    );
}

#[test]
fn the_playback_roles_go_back_to_the_device_the_user_picked_in_crossfx() {
    let mut defaults = all_roles(&speakers().id);
    defaults.set(Role::Communications, Some(microphone_speakers().id));
    let mut h = harness_with(
        vec![
            speakers(),
            headphones(),
            microphone_speakers(),
            fxsound(),
            microphone(),
        ],
        defaults,
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.start();
    h.send(UiToAudio::SelectDevice {
        node_name: headphones().id,
        direction: OUT,
    });
    assert_eq!(
        h.engine.lanes.output.attachment(),
        Some(headphones().id.as_str())
    );
    h.engine.shutdown();
    let after = h.defaults(OUT);
    assert_eq!(
        after.get(Role::Console),
        Some(headphones().id.as_str()),
        "where the user listened through CrossFX, as the original gives it back"
    );
    assert_eq!(after.get(Role::Multimedia), Some(headphones().id.as_str()));
    assert_eq!(
        after.get(Role::Communications),
        Some(microphone_speakers().id.as_str()),
        "calls where they went before"
    );
}

/// The speakers of a USB microphone, as this machine's fifine has them: the communications
/// default the original FxSound left alone.
fn microphone_speakers() -> Endpoint {
    Endpoint {
        enumerator: "USB".to_owned(),
        ..endpoint(
            "{0.0.0.00000000}.{9acba1b5}",
            OUT,
            "Динамики (3- fifine Microphone)",
            "Динамики",
            "3- fifine Microphone",
        )
    }
}

#[test]
fn the_record_is_written_before_a_claim_s_first_write_and_struck_after_its_hand_back() {
    let mut h = harness();
    h.start();
    let all = h.did_all();
    let recorded = all
        .iter()
        .position(|did| *did == Did::Recorded(Some(fxsound().id), None))
        .expect("the claim is recorded");
    let first_write = all
        .iter()
        .position(|did| matches!(did, Did::SetDefault(..)))
        .expect("the claim writes");
    assert!(recorded < first_write, "{all:?}");
    h.forget();
    h.engine.shutdown();
    let all = h.did_all();
    let last_write = all
        .iter()
        .rposition(|did| matches!(did, Did::SetDefault(..)))
        .expect("the hand-back writes");
    let struck = all
        .iter()
        .position(|did| *did == Did::Recorded(None, None))
        .expect("the claim is struck out");
    assert!(struck > last_write, "{all:?}");
    assert_eq!(h.fake().record, PerDirection::default());
}

#[test]
fn a_cable_the_user_made_their_default_is_never_taken_for_one_a_killed_run_left() {
    // FxSound's driver for the speakers, VB-Cable for the microphone — and "CABLE Output" the
    // user's own default recording device, which they record the desktop with. The input lane is
    // detached, as it is at every start until a microphone is picked. At the start nothing tells
    // their default from one Windows put there; CrossFX never held either, and repairs neither.
    let [cable_in, cable_out] = cable();
    let endpoints = vec![
        speakers(),
        fxsound(),
        microphone(),
        cable_in.clone(),
        cable_out.clone(),
    ];
    let mut h = harness_with(
        endpoints.clone(),
        all_roles(&speakers().id),
        all_roles(&cable_out.id),
        &StartOptions::default(),
    );
    h.start();
    for _ in 0..=(REPAIR_PATIENCE.as_millis() / SUPERVISOR_PERIOD.as_millis()) {
        h.tick(SUPERVISOR_PERIOD);
    }
    assert!(
        !h.did()
            .iter()
            .any(|did| matches!(did, Did::SetDefault(IN, ..))),
        "the user's recording default is theirs"
    );
    assert_eq!(h.defaults(IN), all_roles(&cable_out.id));

    // The same machine after a run that was killed holding the microphone's default there: the
    // record says so, and it is given back.
    let mut h = harness_recorded(
        endpoints,
        all_roles(&speakers().id),
        all_roles(&cable_out.id),
        &StartOptions::default(),
        PerDirection {
            output: None,
            input: Some(cable_out.id.clone()),
        },
    );
    h.start();
    for _ in 0..=(REPAIR_PATIENCE.as_millis() / SUPERVISOR_PERIOD.as_millis()) {
        h.tick(SUPERVISOR_PERIOD);
    }
    assert_eq!(h.defaults(IN), all_roles(&microphone().id));
    assert_eq!(h.fake().record.input, None, "and struck from the record");
}

/// The names of the devices the GUI was last offered.
fn offered(h: &Harness) -> Vec<String> {
    h.engine
        .devices()
        .into_iter()
        .map(|device| device.name)
        .collect()
}

/// Whether `messages` say that `name` was the default from before CrossFX, for the settings to
/// keep.
fn remembered(messages: &[AudioToUi], name: &str) -> bool {
    messages.iter().any(|message| {
        matches!(message, AudioToUi::RememberedDefault { node_name, .. } if node_name == name)
    })
}

#[test]
fn a_cable_the_user_plays_into_is_a_device_the_lane_plays_to_and_their_default_comes_back_there() {
    // FxSound's driver and VB-Cable; the user plays everything into "CABLE Input", which a
    // streaming program records from "CABLE Output" — and the settings remember "CABLE Input" as
    // their default from before CrossFX, which is what says it is theirs: its being the default
    // says nothing, Windows giving a cable it has just installed every role by itself.
    let [cable_in, cable_out] = cable();
    let mut h = harness_with(
        vec![
            speakers(),
            fxsound(),
            microphone(),
            cable_in.clone(),
            cable_out.clone(),
        ],
        all_roles(&cable_in.id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.start();
    assert_eq!(
        h.did().first(),
        Some(&Did::Started(OUT, fxsound().id, speakers().id)),
        "before the app has spoken nothing says the cable is the user's: the speakers, not silence"
    );
    assert!(!remembered(&h.messages(), &cable_in.id));
    h.forget();
    // The app's first word: the default from before CrossFX, as the settings keep it.
    h.send(UiToAudio::SeedRememberedDefaults {
        output: cable_in.id.clone(),
        input: String::new(),
    });
    assert_eq!(
        h.did(),
        [
            Did::Stopped(OUT, Duration::ZERO),
            Did::Started(OUT, fxsound().id, cable_in.id.clone())
        ],
        "the lane plays into the cable, as the original would"
    );
    let devices = offered(&h);
    assert!(devices.contains(&cable_in.id) && devices.contains(&cable_out.id));
    assert!(!devices.contains(&fxsound().id));
    // The microphone is not mixed into the user's cable: with no other, it has none, and is told
    // the cable is in use rather than missing.
    h.forget();
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    assert!(
        !h.did()
            .iter()
            .any(|did| matches!(did, Did::Started(IN, ..) | Did::SetDefault(IN, ..))),
        "{:?}",
        h.did()
    );
    assert!(h.messages().contains(&AudioToUi::Error {
        direction: Some(IN),
        message: AudioError::VirtualInputDeviceInUse.to_string(),
    }));
    // Given back where it was.
    h.engine.shutdown();
    assert_eq!(h.defaults(OUT), all_roles(&cable_in.id));
}

/// VB-Cable installed while CrossFX runs, and Windows giving it every role of both directions in
/// the same burst of notifications, as it gives a device just installed the first chance at the
/// default (module docs, "Whose default it is"); and a moment after.
fn windows_gives_a_new_cable_every_role(
    h: &mut Harness,
    cable_in: &Endpoint,
    cable_out: &Endpoint,
) {
    h.fake()
        .endpoints
        .extend([cable_in.clone(), cable_out.clone()]);
    h.fake().defaults.output = all_roles(&cable_in.id);
    h.fake().defaults.input = all_roles(&cable_out.id);
    let mut events = vec![Event::Devices];
    for (direction, id) in [(OUT, &cable_in.id), (IN, &cable_out.id)] {
        events.extend(Role::ALL.into_iter().map(|role| Event::DefaultChanged {
            direction,
            role,
            id: Some(id.clone()),
        }));
    }
    h.burst(events);
    h.tick(SUPERVISOR_PERIOD);
}

#[test]
fn a_cable_windows_makes_the_default_as_it_is_installed_is_no_pick_and_the_microphone_takes_it() {
    // FxSound's driver for the speakers, and the microphone's lane picked with no cable for it
    // yet — until the user installs VB-Cable, as the window tells them to. Whether the speakers'
    // lane follows the system's default or a ranking decides it, Windows' default is no pick.
    let [cable_in, cable_out] = cable();
    let ranked = StartOptions {
        output_priority: DevicePriority {
            names: vec![speakers().id],
            new_devices_first: false,
        },
        input_priority: DevicePriority {
            names: vec![microphone().id],
            new_devices_first: false,
        },
        ..StartOptions::default()
    };
    for options in [StartOptions::default(), ranked] {
        let mut h = harness_with(
            vec![speakers(), fxsound(), microphone()],
            all_roles(&speakers().id),
            all_roles(&microphone().id),
            &options,
        );
        h.start();
        h.send(UiToAudio::SelectDevice {
            node_name: microphone().id,
            direction: IN,
        });
        assert!(h.messages().contains(&AudioToUi::Error {
            direction: Some(IN),
            message: AudioError::NoVirtualInputDevice.to_string(),
        }));
        windows_gives_a_new_cable_every_role(&mut h, &cable_in, &cable_out);
        let messages = h.messages();
        assert!(
            !messages
                .iter()
                .any(|message| matches!(message, AudioToUi::DesktopPick { .. })),
            "{options:?}: Windows' default is not the user's pick: {messages:?}"
        );
        assert!(!remembered(&messages, &cable_in.id) && !remembered(&messages, &cable_out.id));
        assert_eq!(h.engine.memory.output.user_selected, "", "nothing picked");
        assert_eq!(
            h.engine.memory.input.user_selected,
            microphone().id,
            "the microphone the user picked stays theirs"
        );
        // The speakers' lane plays on to the speakers, and has the playback roles back; calls go
        // back to the speakers they were on.
        let output = h
            .engine
            .lanes
            .output
            .stream
            .as_ref()
            .map(|s| s.plan().clone());
        assert_eq!(
            output.map(|plan| (plan.capture, plan.render)),
            Some((fxsound().id, speakers().id))
        );
        assert_eq!(h.defaults(OUT), playback_on(&fxsound().id, &speakers().id));
        // The microphone has its cable at last, its recording default adopted where Windows put
        // it.
        assert_eq!(
            h.engine
                .lanes
                .input
                .claim
                .as_ref()
                .map(|c| c.endpoint.clone()),
            Some(cable_out.id.clone())
        );
        assert_eq!(h.defaults(IN), all_roles(&cable_out.id));
        assert_eq!(
            h.engine.lanes.input.attachment(),
            Some(microphone().id.as_str())
        );
        assert!(
            !messages
                .iter()
                .any(|message| matches!(message, AudioToUi::Error { .. }))
        );
        // Every default on a real device again at the exit.
        h.engine.shutdown();
        assert_eq!(h.defaults(OUT), all_roles(&speakers().id), "{options:?}");
        assert_eq!(h.defaults(IN), all_roles(&microphone().id), "{options:?}");
    }
}

#[test]
fn a_role_windows_moves_from_one_cable_it_installed_to_the_next_is_still_from_the_user_s_device() {
    // The power is off, so CrossFX holds no default and puts nothing back; VB-Cable is installed,
    // then Cable A, and Windows gives each every role in turn.
    let [cable_in, cable_out] = cable();
    let [a_in, a_out] = cable_a();
    let mut h = harness_with(
        vec![speakers(), fxsound(), microphone()],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions {
            want_default: false,
            ..StartOptions::default()
        },
    );
    h.start();
    windows_gives_a_new_cable_every_role(&mut h, &cable_in, &cable_out);
    windows_gives_a_new_cable_every_role(&mut h, &a_in, &a_out);
    assert_eq!(h.did(), [], "nothing written with the power off");
    // The power on: the default from before CrossFX is the speakers, never a cable.
    h.forget();
    h.send(UiToAudio::SetAsDefault {
        direction: OUT,
        want: true,
    });
    let messages = h.messages();
    assert!(
        messages.contains(&AudioToUi::RememberedDefault {
            direction: OUT,
            node_name: speakers().id,
        }),
        "{messages:?}"
    );
    assert!(!remembered(&messages, &cable_in.id) && !remembered(&messages, &a_in.id));
    h.engine.shutdown();
    assert_eq!(
        h.defaults(OUT).get(Role::Multimedia),
        Some(speakers().id.as_str())
    );
}

#[test]
fn a_cable_installed_while_the_microphone_runs_on_another_moves_neither_lane() {
    // The microphone runs on Cable A in a call; VB-Cable is installed meanwhile, and Windows
    // gives it every role.
    let [cable_in, cable_out] = cable();
    let [a_in, a_out] = cable_a();
    let mut h = harness_with(
        vec![
            speakers(),
            fxsound(),
            microphone(),
            a_in.clone(),
            a_out.clone(),
        ],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.start();
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    h.record_from(&a_out.id);
    h.forget();
    windows_gives_a_new_cable_every_role(&mut h, &cable_in, &cable_out);
    assert!(
        !h.messages()
            .iter()
            .any(|message| matches!(message, AudioToUi::DesktopPick { .. }))
    );
    assert_eq!(h.engine.memory.input.user_selected, microphone().id);
    let streams = |h: &Harness| {
        h.engine
            .lanes
            .iter()
            .filter_map(|(_, lane)| lane.stream.as_ref())
            .map(|stream| (stream.plan().capture.clone(), stream.plan().render.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        streams(&h),
        [
            (fxsound().id, speakers().id),
            (microphone().id, a_in.id.clone())
        ],
        "the call still hears the user, and only the user: {:?}",
        h.did()
    );
    assert!(
        !h.did()
            .iter()
            .any(|did| matches!(did, Did::Started(..) | Did::Stopped(..))),
        "{:?}",
        h.did()
    );
    // Every role back where it was: the recorders on Cable A, the speakers' on FxSound's driver,
    // calls on the speakers.
    assert_eq!(h.defaults(IN), all_roles(&a_out.id));
    assert_eq!(h.defaults(OUT), playback_on(&fxsound().id, &speakers().id));
}

#[test]
fn defaults_found_on_a_cable_at_the_start_are_windows_and_the_cable_is_the_microphone_s() {
    // FxSound's driver, and VB-Cable installed before CrossFX started, with every default on it:
    // what Windows does with a device just installed, and nothing says the user wanted it.
    let [cable_in, cable_out] = cable();
    let mut h = harness_with(
        vec![
            speakers(),
            fxsound(),
            microphone(),
            cable_in.clone(),
            cable_out.clone(),
        ],
        all_roles(&cable_in.id),
        all_roles(&cable_out.id),
        &StartOptions::default(),
    );
    h.start();
    assert_eq!(
        h.did().first(),
        Some(&Did::Started(OUT, fxsound().id, speakers().id)),
        "the speakers are played to, not the cable"
    );
    assert!(!remembered(&h.messages(), &cable_in.id));
    // The microphone takes the cable: its recording roles adopted where Windows put them, and the
    // calls' default moved off the side the microphone plays into.
    h.forget();
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    assert!(
        !h.messages()
            .iter()
            .any(|message| matches!(message, AudioToUi::Error { .. }))
    );
    assert_eq!(
        h.engine
            .lanes
            .input
            .claim
            .as_ref()
            .map(|c| c.endpoint.clone()),
        Some(cable_out.id.clone())
    );
    assert_eq!(
        h.did(),
        [Did::SetDefault(OUT, speakers().id, Role::Communications)],
        "nothing else to write: the recording roles are on the cable already"
    );
    assert_eq!(h.defaults(OUT), playback_on(&fxsound().id, &speakers().id));
    // At the exit every default is on a real device.
    h.engine.shutdown();
    assert_eq!(h.defaults(OUT), all_roles(&speakers().id));
    assert_eq!(h.defaults(IN), all_roles(&microphone().id));
}

#[test]
fn a_default_found_on_a_cable_the_start_could_not_list_yet_is_windows_once_it_is_listed() {
    // VB-Cable had every default before CrossFX started, but its endpoints are listed only by the
    // read after the first.
    let [cable_in, cable_out] = cable();
    let mut h = harness_with(
        vec![speakers(), fxsound(), microphone()],
        all_roles(&cable_in.id),
        all_roles(&cable_out.id),
        &StartOptions::default(),
    );
    h.start();
    h.fake()
        .endpoints
        .extend([cable_in.clone(), cable_out.clone()]);
    h.event(Event::Devices);
    h.forget();
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    assert_eq!(
        h.engine
            .lanes
            .input
            .claim
            .as_ref()
            .map(|c| c.endpoint.clone()),
        Some(cable_out.id.clone()),
        "the cable is the microphone's: {:?}",
        h.messages()
    );
    assert_eq!(h.defaults(OUT), playback_on(&fxsound().id, &speakers().id));
    h.engine.shutdown();
    assert_eq!(h.defaults(OUT), all_roles(&speakers().id));
    assert_eq!(h.defaults(IN), all_roles(&microphone().id));
}

#[test]
fn on_a_machine_with_only_a_cable_a_default_windows_put_on_it_is_given_back_to_the_speakers() {
    // No FxSound driver: VB-Cable is the speakers' lane's device, and Windows made it the default
    // for every role as it was installed — nothing says the user did.
    let [cable_in, cable_out] = cable();
    let mut h = harness_with(
        vec![
            speakers(),
            microphone(),
            cable_in.clone(),
            cable_out.clone(),
        ],
        all_roles(&cable_in.id),
        all_roles(&cable_out.id),
        &StartOptions::default(),
    );
    h.start();
    assert_eq!(
        h.did(),
        [Did::Started(OUT, cable_in.id.clone(), speakers().id)],
        "nothing to write: the playback roles are on the cable already"
    );
    assert!(
        h.did_all()
            .contains(&Did::Recorded(Some(cable_in.id.clone()), None)),
        "CrossFX holds them, and a run killed now leaves them for the next start to give back: {:?}",
        h.did_all()
    );
    assert!(!remembered(&h.messages(), &cable_in.id));
    let devices = offered(&h);
    assert!(!devices.contains(&cable_in.id) && !devices.contains(&cable_out.id));
    h.forget();
    h.engine.shutdown();
    assert_eq!(
        h.defaults(OUT),
        all_roles(&speakers().id),
        "given back to the speakers: no silence once CrossFX has quit"
    );
    assert_eq!(h.fake().record.output, None);
    assert_eq!(
        h.defaults(IN),
        all_roles(&microphone().id),
        "and the recording roles, which Windows put on the recording side of CrossFX's own cable \
         — where every recorder records the desktop's sound — are on the microphone again"
    );

    // The same while it runs, once the app has had its say: whatever records, records the
    // microphone, not the desktop.
    let mut h = harness_with(
        vec![
            speakers(),
            microphone(),
            cable_in.clone(),
            cable_out.clone(),
        ],
        all_roles(&cable_in.id),
        all_roles(&cable_out.id),
        &StartOptions::default(),
    );
    h.start();
    h.forget();
    h.send(UiToAudio::SeedRememberedDefaults {
        output: String::new(),
        input: String::new(),
    });
    assert_eq!(h.did(), every_role(IN, &microphone().id), "{:?}", h.did());
    assert_eq!(
        h.defaults(OUT),
        all_roles(&cable_in.id),
        "the playback roles are CrossFX's, and calls heard through it"
    );
}

#[test]
fn a_cable_s_default_is_the_user_s_once_it_moves_there_after_the_cable_has_arrived() {
    // VB-Cable installed while CrossFX runs, and the playback default moved onto it in the burst
    // of its arrival, a moment later, or once the cable has been there a while: only the last is
    // the user making it their default. The speakers' lane follows the system's default.
    let [cable_in, cable_out] = cable();
    for after in [Duration::ZERO, PICK_WAIT / 2, PICK_WAIT + SUPERVISOR_PERIOD] {
        let mut h = harness();
        h.start();
        h.fake()
            .endpoints
            .extend([cable_in.clone(), cable_out.clone()]);
        h.event(Event::Devices);
        if !after.is_zero() {
            h.tick(after);
        }
        h.forget();
        for &role in Role::claimed_by(OUT) {
            h.fake()
                .defaults
                .output
                .set(role, Some(cable_in.id.clone()));
        }
        h.burst(playback_default_changed(&cable_in.id));
        h.tick(SUPERVISOR_PERIOD);
        let picked = after > PICK_WAIT;
        let messages = h.messages();
        assert_eq!(
            messages.contains(&AudioToUi::DesktopPick {
                direction: OUT,
                node_name: cable_in.id.clone(),
            }),
            picked,
            "{after:?}: {messages:?}"
        );
        assert_eq!(
            remembered(&messages, &cable_in.id),
            picked,
            "{after:?}: the user's own default is remembered, Windows' never"
        );
        let target = if picked { &cable_in.id } else { &speakers().id };
        assert_eq!(
            h.engine.lanes.output.attachment(),
            Some(target.as_str()),
            "{after:?}"
        );
        assert_eq!(
            h.defaults(OUT),
            playback_on(&fxsound().id, &speakers().id),
            "{after:?}: the playback roles are CrossFX's either way"
        );
        // The microphone mixes itself into no cable the user plays into, and says so; one Windows
        // made the default by itself is its own.
        h.send(UiToAudio::SelectDevice {
            node_name: microphone().id,
            direction: IN,
        });
        let in_use = h.messages().contains(&AudioToUi::Error {
            direction: Some(IN),
            message: AudioError::VirtualInputDeviceInUse.to_string(),
        });
        assert_eq!(in_use, picked, "{after:?}");
        assert_eq!(h.engine.lanes.input.claim.is_some(), !picked, "{after:?}");
        h.engine.shutdown();
        assert_eq!(
            h.defaults(OUT),
            playback_on(target, &speakers().id),
            "{after:?}: given back where the user listened"
        );
    }
}

#[test]
fn a_cable_s_recording_side_made_the_default_is_never_the_microphone_the_lane_follows() {
    // The microphone runs on VB-Cable, following the system's default; Cable A has long been
    // there, and the user makes "CABLE-A Output" the default for the program that records the
    // desktop with it.
    let [cable_in, cable_out] = cable();
    let [a_in, a_out] = cable_a();
    let mut h = harness_with(
        vec![
            speakers(),
            fxsound(),
            microphone(),
            cable_in.clone(),
            cable_out.clone(),
            a_in,
            a_out.clone(),
        ],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.start();
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    h.record_from(&cable_out.id);
    h.tick(PICK_WAIT);
    h.forget();
    h.fake().defaults.input = all_roles(&a_out.id);
    h.burst(Role::ALL.into_iter().map(|role| Event::DefaultChanged {
        direction: IN,
        role,
        id: Some(a_out.id.clone()),
    }));
    h.tick(SUPERVISOR_PERIOD);
    assert!(
        !h.messages()
            .iter()
            .any(|message| matches!(message, AudioToUi::DesktopPick { .. })),
        "the microphone stays the user's"
    );
    assert_eq!(h.engine.memory.input.user_selected, microphone().id);
    assert_eq!(
        h.engine
            .lanes
            .input
            .stream
            .as_ref()
            .map(|stream| (stream.plan().capture.clone(), stream.plan().render.clone())),
        Some((microphone().id, cable_in.id.clone())),
        "never the recording side of a cable recorded in the microphone's place"
    );
    assert_eq!(h.did(), [], "and the recorders' default is theirs");
    // At the exit too.
    h.engine.shutdown();
    assert_eq!(h.defaults(IN), all_roles(&a_out.id));
}

#[test]
fn a_device_picked_on_the_desktop_with_the_power_off_is_where_the_lane_plays_once_it_is_on() {
    // The power is off: the lane holds no default, and the user picks the headphones in the sound
    // settings. The pick is theirs as much as with the power on (`docs/spec/12-audio-io.md` §21,
    // 4a), and the settings keep it.
    let mut h = harness_with(
        vec![speakers(), headphones(), fxsound(), microphone()],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions {
            want_default: false,
            ..StartOptions::default()
        },
    );
    h.start();
    h.forget();
    for &role in Role::claimed_by(OUT) {
        h.fake().defaults.output.set(role, Some(headphones().id));
    }
    h.burst(playback_default_changed(&headphones().id));
    assert!(h.messages().contains(&AudioToUi::DesktopPick {
        direction: OUT,
        node_name: headphones().id,
    }));
    assert_eq!(h.did(), [], "nothing written with the power off");
    h.send(UiToAudio::SetAsDefault {
        direction: OUT,
        want: true,
    });
    assert_eq!(
        h.did().first(),
        Some(&Did::Started(OUT, fxsound().id, headphones().id)),
        "{:?}",
        h.did()
    );
    h.engine.shutdown();
    assert_eq!(
        h.defaults(OUT),
        playback_on(&headphones().id, &speakers().id)
    );
}

#[test]
fn the_microphone_leaves_alone_a_cable_the_user_records_from_and_adopts_one_a_killed_run_left() {
    let [cable_in, cable_out] = cable();
    let machine = vec![
        speakers(),
        fxsound(),
        microphone(),
        cable_in.clone(),
        cable_out.clone(),
    ];
    // The user records the desktop from "CABLE Output": they made it their default recording
    // device while CrossFX ran, the cable having long been there.
    let mut h = harness_with(
        machine.clone(),
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.start();
    h.tick(PICK_WAIT);
    h.fake().defaults.input = all_roles(&cable_out.id);
    h.burst(Role::ALL.into_iter().map(|role| Event::DefaultChanged {
        direction: IN,
        role,
        id: Some(cable_out.id.clone()),
    }));
    h.forget();
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    assert_eq!(h.did(), [], "{:?}", h.did());
    assert!(h.messages().contains(&AudioToUi::Error {
        direction: Some(IN),
        message: AudioError::VirtualInputDeviceInUse.to_string(),
    }));
    assert_eq!(h.defaults(IN), all_roles(&cable_out.id));

    // The same default found there at the start, with the settings remembering it as theirs from
    // before CrossFX: theirs as much.
    let mut h = harness_with(
        machine.clone(),
        all_roles(&speakers().id),
        all_roles(&cable_out.id),
        &StartOptions::default(),
    );
    h.start();
    h.send(UiToAudio::SeedRememberedDefaults {
        output: String::new(),
        input: cable_out.id.clone(),
    });
    h.forget();
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    assert_eq!(h.did(), [], "{:?}", h.did());
    assert!(h.messages().contains(&AudioToUi::Error {
        direction: Some(IN),
        message: AudioError::VirtualInputDeviceInUse.to_string(),
    }));

    // A run of CrossFX was killed holding the microphone's default there: the record says so,
    // and the lane attached at the start adopts it, and gives it back at the end.
    let mut h = harness_recorded(
        machine,
        all_roles(&speakers().id),
        all_roles(&cable_out.id),
        &StartOptions::default(),
        PerDirection {
            output: None,
            input: Some(cable_out.id.clone()),
        },
    );
    h.start();
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    // The call it was killed in is still being recorded.
    h.record_from(&cable_out.id);
    assert!(
        h.did()
            .contains(&Did::Started(IN, microphone().id, cable_in.id.clone())),
        "{:?}",
        h.did()
    );
    h.engine.shutdown();
    assert_eq!(h.defaults(IN), all_roles(&microphone().id));
    assert_eq!(h.fake().record.input, None);
}

#[test]
fn a_cable_installed_while_the_speakers_follow_the_system_is_no_device_just_added_to_them() {
    // FxSound's driver for the speakers' lane, which follows the system's default with nothing
    // picked in CrossFX — the state after a fresh install. VB-Cable is installed meanwhile, for the
    // microphone.
    let [cable_in, cable_out] = cable();
    let mut h = harness();
    h.start();
    h.forget();
    h.fake()
        .endpoints
        .extend([cable_in.clone(), cable_out.clone()]);
    h.event(Event::Devices);
    h.tick(SUPERVISOR_PERIOD);
    assert_eq!(h.did(), [], "the speakers' lane plays on where it played");
    // Offered all the same, both sides: the user's to pick.
    let devices = offered(&h);
    assert!(devices.contains(&cable_in.id) && devices.contains(&cable_out.id));

    // The microphone has it: the speakers' lane does not play into it.
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    assert_eq!(h.did(), claimed(IN, &cable_out.id), "{:?}", h.did());
    assert!(!h.messages().contains(&AudioToUi::Error {
        direction: Some(IN),
        message: AudioError::NoVirtualInputDevice.to_string(),
    }));
    // And at the exit every default goes back where it was.
    h.engine.shutdown();
    assert_eq!(h.defaults(OUT), all_roles(&speakers().id));
    assert_eq!(h.defaults(IN), all_roles(&microphone().id));
}

#[test]
fn a_blind_start_takes_a_real_device_for_the_first_there_is_not_a_cable_listed_before_it() {
    // A killed original left the default on its driver, nothing is remembered, and Windows lists
    // VB-Cable before the speakers, as the IDs of its endpoints may sort.
    let [cable_in, cable_out] = cable();
    let machine = vec![cable_in, cable_out, speakers(), fxsound(), microphone()];
    let mut h = harness_with(
        machine.clone(),
        all_roles(&fxsound().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.start();
    assert_eq!(
        h.did().first(),
        Some(&Did::Started(OUT, fxsound().id, speakers().id)),
        "{:?}",
        h.did()
    );
    h.engine.shutdown();
    assert_eq!(h.defaults(OUT), all_roles(&speakers().id));

    // The power is off: the repair at the start gives the default to the speakers too.
    let mut h = harness_with(
        machine,
        all_roles(&fxsound().id),
        all_roles(&microphone().id),
        &StartOptions {
            want_default: false,
            ..StartOptions::default()
        },
    );
    h.start();
    for _ in 0..=(REPAIR_PATIENCE.as_millis() / SUPERVISOR_PERIOD.as_millis()) {
        h.tick(SUPERVISOR_PERIOD);
    }
    assert_eq!(h.did(), every_role(OUT, &speakers().id));
}

#[test]
fn the_microphone_s_last_resort_is_a_real_microphone_not_a_cable_listed_before_it() {
    // FxSound's driver for the speakers, VB-Cable for the microphone, and Cable A free — listed
    // first. The user records from their USB microphone, their default, through CrossFX.
    let [cable_in, cable_out] = cable();
    let [cable_a_in, cable_a_out] = cable_a();
    let mut h = harness_with(
        vec![
            cable_a_in,
            cable_a_out.clone(),
            speakers(),
            fxsound(),
            headset_microphone(),
            microphone(),
            cable_in.clone(),
            cable_out.clone(),
        ],
        all_roles(&speakers().id),
        all_roles(&headset_microphone().id),
        &StartOptions::default(),
    );
    h.start();
    h.send(UiToAudio::SelectDevice {
        node_name: headset_microphone().id,
        direction: IN,
    });
    h.record_from(&cable_out.id);
    assert!(
        h.did().contains(&Did::Started(
            IN,
            headset_microphone().id,
            cable_in.id.clone()
        )),
        "{:?}",
        h.did()
    );
    // It is unplugged: nothing remembered is left, and the lane takes the first microphone there
    // is — the one in the laptop, never the recording side of a cable.
    h.forget();
    h.fake()
        .endpoints
        .retain(|endpoint| endpoint.id != headset_microphone().id);
    h.event(Event::Devices);
    assert!(
        h.did()
            .contains(&Did::Started(IN, microphone().id, cable_in.id.clone())),
        "{:?}",
        h.did()
    );
    assert!(
        !h.did()
            .iter()
            .any(|did| matches!(did, Did::Started(IN, capture, _) if *capture == cable_a_out.id)),
        "{:?}",
        h.did()
    );
}

#[test]
fn a_cable_the_user_picks_in_crossfx_is_played_to_and_given_the_default_back() {
    let [cable_in, cable_out] = cable();
    let mut h = harness_with(
        vec![
            speakers(),
            fxsound(),
            microphone(),
            cable_in.clone(),
            cable_out,
        ],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.start();
    h.forget();
    h.send(UiToAudio::SelectDevice {
        node_name: cable_in.id.clone(),
        direction: OUT,
    });
    assert_eq!(
        h.did(),
        [
            Did::Stopped(OUT, Duration::ZERO),
            Did::Started(OUT, fxsound().id, cable_in.id.clone())
        ]
    );
    h.engine.shutdown();
    assert_eq!(h.defaults(OUT), playback_on(&cable_in.id, &speakers().id));
}

#[test]
fn with_a_ranking_a_cable_the_user_makes_the_default_is_no_newcomer_to_take_the_lane() {
    // New devices go first, and the ranking does not name VB-Cable: were it a device just added
    // once the user made it the default, it would take the lane from the speakers.
    let [cable_in, cable_out] = cable();
    let mut h = harness_with(
        vec![
            speakers(),
            fxsound(),
            microphone(),
            cable_in.clone(),
            cable_out,
        ],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions {
            output_priority: DevicePriority {
                names: vec![speakers().id],
                new_devices_first: true,
            },
            ..StartOptions::default()
        },
    );
    h.start();
    assert_eq!(
        h.did().first(),
        Some(&Did::Started(OUT, fxsound().id, speakers().id))
    );
    h.forget();
    // A streaming program's setup: the sound settings make "CABLE Input" the default.
    for role in [Role::Console, Role::Multimedia] {
        h.fake()
            .defaults
            .output
            .set(role, Some(cable_in.id.clone()));
        h.event(Event::DefaultChanged {
            direction: OUT,
            role,
            id: Some(cable_in.id.clone()),
        });
    }
    h.event(Event::Devices);
    h.tick(SUPERVISOR_PERIOD);
    assert_eq!(h.did(), [], "the lane stays on the speakers");
    h.engine.shutdown();
    assert_eq!(
        h.defaults(OUT).get(Role::Multimedia),
        Some(cable_in.id.as_str()),
        "and the user's default stays theirs"
    );
}

#[test]
fn a_default_a_killed_run_left_on_a_cable_the_speakers_no_longer_stand_on_is_not_the_user_s() {
    // A run on VB-Cable was killed holding the default there; FxSound's driver has been installed
    // since, and the speakers' lane stands on it now. The cable's default is the record's.
    let [cable_in, cable_out] = cable();
    let mut h = harness_recorded(
        vec![
            speakers(),
            fxsound(),
            microphone(),
            cable_in.clone(),
            cable_out,
        ],
        playback_on(&cable_in.id, &speakers().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
        PerDirection {
            output: Some(cable_in.id.clone()),
            input: None,
        },
    );
    h.start();
    assert_eq!(
        h.did().first(),
        Some(&Did::Started(OUT, fxsound().id, speakers().id)),
        "{:?}",
        h.did()
    );
    h.engine.shutdown();
    assert_eq!(h.defaults(OUT), all_roles(&speakers().id));
    assert_eq!(h.fake().record.output, None);
}

#[test]
fn the_original_fxsound_moving_the_default_is_no_pick_of_the_user_s() {
    let mut h = harness_with(
        vec![speakers(), headphones(), fxsound(), microphone()],
        all_roles(&fxsound().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.fake().original = Some("FxSound.exe (process 12364)".to_owned());
    h.start();
    h.forget();
    // It quits, and gives its default back to the headphones it played to.
    h.fake().defaults.output = all_roles(&headphones().id);
    for role in [Role::Console, Role::Multimedia] {
        h.event(Event::DefaultChanged {
            direction: OUT,
            role,
            id: Some(headphones().id),
        });
    }
    assert!(
        !h.messages()
            .iter()
            .any(|message| matches!(message, AudioToUi::DesktopPick { .. })),
        "the original's hand-back is not the user's pick"
    );
    h.fake().original = None;
    h.tick(ORIGINAL_EVERY);
    assert_eq!(
        h.did().first(),
        Some(&Did::Started(OUT, fxsound().id, headphones().id)),
        "the lane takes over where the original played"
    );
}

#[test]
fn once_the_original_fxsound_is_gone_a_default_it_was_killed_holding_is_given_back() {
    // The power is off, and the original holds the default when CrossFX starts.
    let mut h = harness_with(
        vec![speakers(), fxsound(), microphone()],
        all_roles(&fxsound().id),
        all_roles(&microphone().id),
        &StartOptions {
            want_default: false,
            ..StartOptions::default()
        },
    );
    h.fake().original = Some("FxSound.exe (process 12364)".to_owned());
    h.start();
    for _ in 0..=(REPAIR_PATIENCE.as_millis() / SUPERVISOR_PERIOD.as_millis()) {
        h.tick(SUPERVISOR_PERIOD);
    }
    assert_eq!(h.did(), [], "the original's default is the original's");
    // It is killed: its driver is still the default, and nothing plays it.
    h.fake().original = None;
    h.tick(ORIGINAL_EVERY);
    for _ in 0..=(REPAIR_PATIENCE.as_millis() / SUPERVISOR_PERIOD.as_millis()) {
        h.tick(SUPERVISOR_PERIOD);
    }
    assert_eq!(h.did(), every_role(OUT, &speakers().id));
}

#[test]
fn a_default_a_killed_run_left_on_the_virtual_device_is_given_back_once_the_app_has_spoken() {
    // The power was left off, and a run before this one was killed holding the default.
    let mut h = harness_with(
        vec![speakers(), headphones(), fxsound(), microphone()],
        all_roles(&fxsound().id),
        all_roles(&microphone().id),
        &StartOptions {
            want_default: false,
            ..StartOptions::default()
        },
    );
    h.start();
    h.send(UiToAudio::SeedRememberedDefaults {
        output: headphones().id,
        input: String::new(),
    });
    assert_eq!(h.did(), [], "not before the app has had its moment");
    for _ in 0..(REPAIR_PATIENCE.as_millis() / SUPERVISOR_PERIOD.as_millis()) {
        h.tick(SUPERVISOR_PERIOD);
    }
    assert_eq!(h.did(), every_role(OUT, &headphones().id));
    assert_eq!(h.defaults(OUT), all_roles(&headphones().id));
    // Once.
    h.forget();
    h.tick(REPAIR_PATIENCE);
    assert_eq!(h.did(), []);
}

#[test]
fn a_default_a_killed_run_left_on_the_virtual_device_is_adopted_by_a_lane_that_takes_it() {
    let mut h = harness_with(
        vec![speakers(), fxsound(), microphone()],
        all_roles(&fxsound().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.start();
    assert_eq!(
        h.did(),
        [Did::Started(OUT, fxsound().id, speakers().id)],
        "every role is on the virtual device already: nothing to write, nothing to remember"
    );
    assert!(
        !h.messages()
            .iter()
            .any(|message| matches!(message, AudioToUi::RememberedDefault { .. }))
    );
    h.tick(REPAIR_PATIENCE);
    // And at the exit it goes to the device the lane played to.
    h.forget();
    h.engine.shutdown();
    assert_eq!(h.defaults(OUT), all_roles(&speakers().id));
}

#[test]
fn a_stream_whose_device_went_has_the_endpoints_read_again_and_moves_keeping_the_default() {
    let mut h = harness_with(
        vec![speakers(), headphones(), fxsound(), microphone()],
        all_roles(&headphones().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.start();
    assert_eq!(
        h.did().first(),
        Some(&Did::Started(OUT, fxsound().id, headphones().id))
    );
    h.forget();
    // The headphones are unplugged: the endpoint goes, and the stream ends with it.
    h.fake()
        .endpoints
        .retain(|endpoint| endpoint.id != headphones().id);
    if let Some((ended, outcome)) = h.fake().ended.output.as_ref() {
        *outcome.lock().expect("the outcome") = Outcome::DeviceGone;
        ended.store(true, Ordering::Relaxed);
    }
    let generation = h.engine.generation;
    h.event(Event::StreamEnded {
        direction: OUT,
        generation,
    });
    assert_eq!(
        h.did(),
        [Did::Started(OUT, fxsound().id, speakers().id)],
        "on the speakers, the default never moved"
    );
    assert!(
        !h.messages()
            .iter()
            .any(|message| matches!(message, AudioToUi::Error { .. })),
        "a device unplugged is not an error"
    );
}

#[test]
fn a_stream_that_will_not_start_is_reported_once_and_retried_with_backoff() {
    let mut h = harness();
    h.fake().refuse_streams = true;
    h.start();
    assert!(h.messages().contains(&AudioToUi::Error {
        direction: Some(OUT),
        message: AudioError::DeviceUnavailable.to_string(),
    }));
    assert_eq!(h.did(), [], "no default for a lane that does not run");
    // Retried when the backoff is up, and still failing, not reported again.
    h.tick(SUPERVISOR_PERIOD);
    h.tick(Duration::from_millis(400));
    assert!(
        !h.messages()
            .iter()
            .any(|message| matches!(message, AudioToUi::Error { .. }))
    );
    // The device comes good.
    h.fake().refuse_streams = false;
    h.tick(Duration::from_secs(6));
    assert_eq!(
        h.did().first(),
        Some(&Did::Started(OUT, fxsound().id, speakers().id))
    );
}

#[test]
fn endpoints_that_cannot_be_read_are_read_again_until_they_can_and_no_device_is_called_missing() {
    let mut h = harness();
    h.fake().service_gone = true;
    h.start();
    let service = AudioToUi::Error {
        direction: None,
        message: AudioError::WindowsAudioUnavailable("the RPC server is unavailable".to_owned())
            .to_string(),
    };
    let errors = |messages: Vec<AudioToUi>| -> Vec<AudioToUi> {
        messages
            .into_iter()
            .filter(|message| matches!(message, AudioToUi::Error { .. }))
            .collect()
    };
    assert_eq!(
        errors(h.messages()),
        [service],
        "the service's error, and no lane's: nothing is known to be missing"
    );
    assert_eq!(h.did(), []);
    // Still gone: read again with the backoff, and said once.
    for _ in 0..10 {
        h.tick(SUPERVISOR_PERIOD);
    }
    assert_eq!(errors(h.messages()), []);
    assert_eq!(h.did(), []);
    // Back: read again with nothing else asking, and the output lane starts as it would have.
    h.fake().service_gone = false;
    h.tick(Duration::from_secs(6));
    let did = h.did();
    assert_eq!(
        did.first(),
        Some(&Did::Started(OUT, fxsound().id, speakers().id)),
        "{did:?}"
    );
    assert!(
        h.messages()
            .iter()
            .any(|message| matches!(message, AudioToUi::Devices(devices) if devices.len() == 2))
    );
}

#[test]
fn a_stream_stuck_in_wasapi_keeps_the_lane_waiting_not_dead_and_it_moves_once_it_is_back() {
    let mut h = harness_with(
        vec![speakers(), headphones(), fxsound(), microphone()],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.start();
    h.forget();
    let (release, stuck) = crossbeam_channel::bounded::<()>(0);
    if let Ok(mut slot) = h.fake().stuck.lock() {
        *slot = Some(stuck);
    }
    h.send(UiToAudio::SelectDevice {
        node_name: headphones().id,
        direction: OUT,
    });
    assert_eq!(
        h.did(),
        [Did::Stopped(OUT, Duration::ZERO)],
        "nothing started without the DSP"
    );
    assert!(
        !h.messages()
            .iter()
            .any(|message| matches!(message, AudioToUi::Error { .. })),
        "a lane waiting for its DSP has nothing to report"
    );
    assert!(h.engine.lanes.output.dsp.is_none());
    // WASAPI lets the thread go.
    drop(release);
    while !h
        .engine
        .lanes
        .output
        .left_behind
        .as_ref()
        .is_some_and(LeftBehind::has_ended)
    {
        std::thread::yield_now();
    }
    h.tick(SUPERVISOR_PERIOD);
    assert!(h.engine.lanes.output.left_behind.is_none());
    assert_eq!(
        h.did()[1..],
        [Did::Started(OUT, fxsound().id, headphones().id)],
        "the user's pick, kept for when the DSP came back"
    );
}

#[test]
fn messages_with_no_windows_meaning_are_taken_and_change_nothing() {
    let mut h = harness();
    h.start();
    h.forget();
    h.send(UiToAudio::SetAppRoutes(Vec::new()));
    h.send(UiToAudio::SeedTargetVolumes(Vec::new()));
    assert_eq!(h.did(), []);
    assert_eq!(h.messages(), []);
    h.send(UiToAudio::SetEchoCancel(true));
    assert_eq!(
        h.messages(),
        [AudioToUi::EchoCancel {
            running: false,
            detail: "echo cancellation is not available on Windows".to_owned(),
        }],
        "answered, as the message promises"
    );
}

#[test]
fn the_sleep_silences_both_lanes_and_the_wake_rebuilds_them_silent_until_attached() {
    let mut h = harness();
    h.start();
    h.send(UiToAudio::SystemSleeping(true));
    let muted = |h: &Harness| {
        h.engine
            .lanes
            .output
            .stream
            .as_ref()
            .map(|stream| stream.shared().system_muted())
    };
    assert_eq!(muted(&h), Some(true));
    h.forget();
    h.send(UiToAudio::SystemSleeping(false));
    let did = h.did();
    assert_eq!(
        did,
        [
            Did::Stopped(OUT, Duration::ZERO),
            Did::Started(OUT, fxsound().id, speakers().id)
        ],
        "rebuilt, the default kept"
    );
    assert_eq!(muted(&h), Some(true), "silent until attached");
    h.tick(SUPERVISOR_PERIOD);
    assert_eq!(muted(&h), Some(false));
}

#[test]
fn the_virtual_device_s_volume_is_what_the_output_lane_plays_at() {
    let mut h = harness();
    h.fake().gain = Some(0.5);
    h.start();
    let gain = |h: &Harness| {
        h.engine
            .lanes
            .output
            .stream
            .as_ref()
            .map(|stream| stream.shared().gain())
    };
    assert_eq!(gain(&h), Some(0.5));
    h.engine.lanes.output.volume = Some(Box::new(Volume(0.25)));
    h.event(Event::Volume(OUT));
    assert_eq!(gain(&h), Some(0.25));
}

#[test]
fn one_bluetooth_headset_on_both_lanes_is_warned_of_once() {
    let headset_out = Endpoint {
        enumerator: "BTHENUM".to_owned(),
        container: Some("{headset}".to_owned()),
        form_factor: Some(3),
        ..endpoint(
            "{0.0.0.00000000}.{bt-a2dp}",
            OUT,
            "Наушники (HECATE G2000 PRO)",
            "Наушники",
            "HECATE G2000 PRO",
        )
    };
    let headset_in = Endpoint {
        enumerator: "BTHHFENUM".to_owned(),
        container: Some("{headset}".to_owned()),
        form_factor: Some(5),
        ..endpoint(
            "{0.0.1.00000000}.{bt-hfp}",
            IN,
            "Головной телефон (HECATE G2000 PRO Hands-Free)",
            "Головной телефон",
            "HECATE G2000 PRO Hands-Free",
        )
    };
    let [cable_in, cable_out] = cable();
    let mut h = harness_with(
        vec![
            headset_out.clone(),
            headset_in.clone(),
            fxsound(),
            cable_in,
            cable_out,
        ],
        all_roles(&headset_out.id),
        all_roles(&headset_in.id),
        &StartOptions::default(),
    );
    h.start();
    h.send(UiToAudio::SelectDevice {
        node_name: headset_in.id.clone(),
        direction: IN,
    });
    h.tick(SUPERVISOR_PERIOD);
    h.tick(SUPERVISOR_PERIOD);
    let warnings: Vec<AudioToUi> = h
        .messages()
        .into_iter()
        .filter(|message| matches!(message, AudioToUi::Warning { .. }))
        .collect();
    assert_eq!(
        warnings,
        [AudioToUi::Warning {
            direction: None,
            message: crossfx_core::i18n::tr(ONE_HEADSET_ON_BOTH_LANES),
        }]
    );
}

#[test]
fn a_new_voice_chain_is_built_in_place_while_the_lane_is_off_and_handed_over_while_it_runs() {
    let [cable_in, cable_out] = cable();
    let mut h = harness_with(
        vec![
            speakers(),
            fxsound(),
            microphone(),
            cable_in,
            cable_out.clone(),
        ],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.start();
    h.send(UiToAudio::SetInputChain("podcast".to_owned()));
    let spec = |h: &mut Harness| {
        h.engine
            .lanes
            .input
            .dsp
            .as_mut()
            .and_then(LaneDsp::as_input_mut)
            .map(|dsp| dsp.engine().spec())
    };
    assert_eq!(spec(&mut h), Some(ChainSpec::podcast()), "in place");
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    h.record_from(&cable_out.id);
    assert!(h.engine.lanes.input.stream.is_some());
    h.send(UiToAudio::SetInputChain("broadcast".to_owned()));
    assert!(
        h.engine.handover.replacement.is_full(),
        "built here and sent to the stream"
    );
    assert!(!h.engine.input_spec_pending);
}

// ---------------------------------------------------------------------------------------------
// Whose a default on a cable is: the evidence, and Windows' defaults put back
// ---------------------------------------------------------------------------------------------

/// Both lanes ranked, as the app's settings are by default: a lane's device the ranking decides,
/// the speakers and the laptop's microphone first.
fn both_ranked() -> StartOptions {
    StartOptions {
        output_priority: DevicePriority {
            names: vec![speakers().id],
            new_devices_first: false,
        },
        input_priority: DevicePriority {
            names: vec![microphone().id],
            new_devices_first: false,
        },
        ..StartOptions::default()
    }
}

/// The streams the lanes run, as (capture, render).
fn streams(h: &Harness) -> Vec<(String, String)> {
    h.engine
        .lanes
        .iter()
        .filter_map(|(_, lane)| lane.stream.as_ref())
        .map(|stream| (stream.plan().capture.clone(), stream.plan().render.clone()))
        .collect()
}

/// Every device the rules of `direction` remember, the user's word and their own choices alike.
fn remembered_slots(h: &Harness, direction: DeviceDirection) -> Vec<String> {
    let memory = h.engine.memory.get(direction);
    [
        &memory.user_selected,
        &memory.most_recent_playback,
        &memory.most_recent_default,
        &memory.prior_default,
        &memory.original_default,
    ]
    .into_iter()
    .filter(|slot| !slot.is_empty())
    .cloned()
    .collect()
}

/// The endpoint the input lane holds its claim on.
fn input_claim(h: &Harness) -> Option<String> {
    h.engine
        .lanes
        .input
        .claim
        .as_ref()
        .map(|claim| claim.endpoint.clone())
}

/// Whether `messages` hold an error of any lane's, or a pick the desktop made.
fn any_error_or_pick(messages: &[AudioToUi]) -> bool {
    messages.iter().any(|message| {
        matches!(
            message,
            AudioToUi::Error { .. } | AudioToUi::DesktopPick { .. }
        )
    })
}

#[test]
fn a_cable_installed_while_the_original_fxsound_runs_is_windows_and_the_microphone_s() {
    // This machine: the original FxSound runs and holds the playback roles on its driver, calls on
    // the speakers. CrossFX's microphone lane is on, with no cable yet; the user installs
    // VB-Cable, Windows gives it every role, and the original takes its two back onto its driver.
    // Then the original quits, handing its default back to the speakers.
    let [cable_in, cable_out] = cable();
    for options in [StartOptions::default(), both_ranked()] {
        let mut h = harness_with(
            vec![speakers(), fxsound(), microphone()],
            playback_on(&fxsound().id, &speakers().id),
            all_roles(&microphone().id),
            &options,
        );
        h.fake().original = Some("FxSound.exe (process 12364)".to_owned());
        h.start();
        h.send(UiToAudio::SelectDevice {
            node_name: microphone().id,
            direction: IN,
        });
        h.forget();
        windows_gives_a_new_cable_every_role(&mut h, &cable_in, &cable_out);
        // The microphone has the cable at once, and Windows' playback roles on it go back where
        // they were: the original's two onto its driver, where it plays from.
        assert_eq!(input_claim(&h), Some(cable_out.id.clone()), "{options:?}");
        assert_eq!(
            h.defaults(OUT),
            playback_on(&fxsound().id, &speakers().id),
            "{options:?}: {:?}",
            h.did()
        );
        for &role in Role::claimed_by(OUT) {
            h.fake().defaults.output.set(role, Some(fxsound().id));
        }
        h.burst(playback_default_changed(&fxsound().id));
        h.tick(SUPERVISOR_PERIOD);
        let messages = h.messages();
        assert!(
            !any_error_or_pick(&messages),
            "{options:?}: the cable is no use of the user's: {messages:?}"
        );
        assert_eq!(input_claim(&h), Some(cable_out.id.clone()), "{options:?}");
        assert_eq!(
            h.defaults(OUT),
            playback_on(&fxsound().id, &speakers().id),
            "{options:?}: calls back on the speakers, the original's two on its driver"
        );
        assert!(!h.engine.in_users_use(&cable_in.id), "{options:?}");
        // The held speakers' lane took no cable for its device, and remembers none.
        assert_eq!(
            h.engine.lanes.output.target.as_deref(),
            Some(speakers().id.as_str()),
            "{options:?}"
        );
        assert!(
            !remembered_slots(&h, OUT).contains(&cable_in.id),
            "{options:?}: {:?}",
            h.engine.memory.output
        );

        // The original quits.
        h.forget();
        h.fake().original = None;
        for &role in Role::claimed_by(OUT) {
            h.fake().defaults.output.set(role, Some(speakers().id));
        }
        h.tick(ORIGINAL_EVERY);
        h.burst(playback_default_changed(&speakers().id));
        h.tick(REPAIR_PATIENCE);
        assert_eq!(
            streams(&h),
            [(fxsound().id, speakers().id)],
            "{options:?}: the speakers, never the cable"
        );
        assert!(!any_error_or_pick(&h.messages()), "{options:?}");
        h.engine.shutdown();
        assert_eq!(h.defaults(OUT), all_roles(&speakers().id), "{options:?}");
        assert_eq!(h.defaults(IN), all_roles(&microphone().id), "{options:?}");
    }
}

#[test]
fn the_rules_own_choice_of_a_cable_never_makes_it_the_user_s() {
    // What the rules remember having played to, or found the default on, is no word of the
    // user's: a cable they came to by mistake would be the user's for good.
    let [cable_in, cable_out] = cable();
    let mut h = harness_with(
        vec![
            speakers(),
            fxsound(),
            microphone(),
            cable_in.clone(),
            cable_out,
        ],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.start();
    let memory = &mut h.engine.memory.output;
    memory.most_recent_playback.clone_from(&cable_in.id);
    memory.most_recent_default.clone_from(&cable_in.id);
    memory.prior_default.clone_from(&cable_in.id);
    memory.original_default.clone_from(&cable_in.id);
    assert!(!h.engine.remembers(OUT, &cable_in.id));
    assert!(!h.engine.chosen(OUT, &cable_in.id));
    // The user's word is: a pick, or the settings'.
    h.engine
        .memory
        .output
        .user_selected
        .clone_from(&cable_in.id);
    assert!(h.engine.chosen(OUT, &cable_in.id));
    h.engine.memory.output.user_selected.clear();
    h.send(UiToAudio::SeedRememberedDefaults {
        output: cable_in.id.clone(),
        input: String::new(),
    });
    assert!(h.engine.chosen(OUT, &cable_in.id));
}

#[test]
fn a_cable_listed_only_after_its_wait_is_windows_once_it_is_listed() {
    // VB-Cable installed while CrossFX runs, Windows' default heard at once and the cable's
    // endpoints readable only after PICK_WAIT: no pick was decided, and the default is Windows'.
    // Calls were on the headphones, which the lane's hand-back would not know to go back to.
    let [cable_in, cable_out] = cable();
    for options in [StartOptions::default(), both_ranked()] {
        let mut h = harness_with(
            vec![speakers(), headphones(), fxsound(), microphone()],
            playback_on(&speakers().id, &headphones().id),
            all_roles(&microphone().id),
            &options,
        );
        h.start();
        h.send(UiToAudio::SelectDevice {
            node_name: microphone().id,
            direction: IN,
        });
        h.forget();
        h.fake().defaults.output = all_roles(&cable_in.id);
        h.fake().defaults.input = all_roles(&cable_out.id);
        let mut events = vec![Event::Devices];
        for (direction, id) in [(OUT, &cable_in.id), (IN, &cable_out.id)] {
            events.extend(Role::ALL.into_iter().map(|role| Event::DefaultChanged {
                direction,
                role,
                id: Some(id.clone()),
            }));
        }
        h.burst(events);
        for _ in 0..=(PICK_WAIT.as_millis() / SUPERVISOR_PERIOD.as_millis()) {
            h.tick(SUPERVISOR_PERIOD);
        }
        assert!(h.engine.pending.output.is_empty() && h.engine.pending.input.is_empty());
        h.fake()
            .endpoints
            .extend([cable_in.clone(), cable_out.clone()]);
        h.event(Event::Devices);
        h.tick(SUPERVISOR_PERIOD);
        let messages = h.messages();
        assert!(!any_error_or_pick(&messages), "{options:?}: {messages:?}");
        assert!(!remembered(&messages, &cable_in.id) && !remembered(&messages, &cable_out.id));
        assert_eq!(
            h.defaults(OUT),
            playback_on(&fxsound().id, &headphones().id),
            "{options:?}: the playback roles back, calls back on the headphones"
        );
        assert_eq!(
            streams(&h),
            [(fxsound().id, speakers().id)],
            "{options:?}: the microphone waits on its cable for a recorder"
        );
        assert_eq!(input_claim(&h), Some(cable_out.id.clone()), "{options:?}");
        h.engine.shutdown();
        assert_eq!(
            h.defaults(OUT),
            playback_on(&speakers().id, &headphones().id),
            "{options:?}"
        );
        assert_eq!(h.defaults(IN), all_roles(&microphone().id), "{options:?}");
    }

    // A real device read late is no cable, and nothing of Windows' to put back: the rules take it
    // for the device just added that it is.
    let mut h = harness();
    h.start();
    for &role in Role::claimed_by(OUT) {
        h.fake().defaults.output.set(role, Some(usb_headset().id));
    }
    h.burst(playback_default_changed(&usb_headset().id));
    for _ in 0..=(PICK_WAIT.as_millis() / SUPERVISOR_PERIOD.as_millis()) {
        h.tick(SUPERVISOR_PERIOD);
    }
    assert!(h.engine.placed.output.get(Role::Console).is_some());
    h.fake().endpoints.push(usb_headset());
    h.event(Event::Devices);
    h.tick(SUPERVISOR_PERIOD);
    assert_eq!(h.engine.placed.output, Placements::default());
    assert_eq!(streams(&h), [(fxsound().id, usb_headset().id)]);
    assert_eq!(h.defaults(OUT), playback_on(&fxsound().id, &speakers().id));
}

#[test]
fn a_role_windows_moves_off_a_cable_it_had_put_it_on_never_goes_back_onto_that_cable() {
    // VB-Cable's defaults found at the start, then Cable A installed while CrossFX runs: Windows
    // moves the calls' role from one cable to the next. It goes to a real device, never back onto
    // VB-Cable — which would make that cable the user's in use, and the microphone avoid it.
    let [cable_in, cable_out] = cable();
    let [a_in, a_out] = cable_a();
    let mut h = harness_with(
        vec![
            speakers(),
            fxsound(),
            microphone(),
            cable_in.clone(),
            cable_out.clone(),
        ],
        all_roles(&cable_in.id),
        all_roles(&cable_out.id),
        &StartOptions::default(),
    );
    h.start();
    h.forget();
    windows_gives_a_new_cable_every_role(&mut h, &a_in, &a_out);
    assert!(
        !h.did().contains(&Did::SetDefault(
            OUT,
            cable_in.id.clone(),
            Role::Communications
        )),
        "{:?}",
        h.did()
    );
    assert_eq!(h.defaults(OUT), playback_on(&fxsound().id, &speakers().id));
    assert!(!h.engine.in_users_use(&cable_in.id));
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    assert_eq!(
        input_claim(&h),
        Some(cable_out.id.clone()),
        "VB-Cable, which sorts first, is the microphone's"
    );
    h.engine.shutdown();
    assert_eq!(h.defaults(OUT), all_roles(&speakers().id));
    assert_eq!(h.defaults(IN), all_roles(&microphone().id));

    // VB-Cable installed with the power off, the power on, then Cable A: where Windows put the
    // calls is followed back to the speakers they were on.
    let mut h = harness_with(
        vec![speakers(), fxsound(), microphone()],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions {
            want_default: false,
            ..StartOptions::default()
        },
    );
    h.start();
    windows_gives_a_new_cable_every_role(&mut h, &cable_in, &cable_out);
    h.send(UiToAudio::SetAsDefault {
        direction: OUT,
        want: true,
    });
    assert_eq!(
        h.defaults(OUT),
        playback_on(&fxsound().id, &speakers().id),
        "CrossFX in the playback roles' path: calls off the cable too"
    );
    h.forget();
    windows_gives_a_new_cable_every_role(&mut h, &a_in, &a_out);
    assert!(
        !h.did()
            .iter()
            .any(|did| matches!(did, Did::SetDefault(OUT, id, _) if *id == cable_in.id)),
        "{:?}",
        h.did()
    );
    assert_eq!(h.defaults(OUT), playback_on(&fxsound().id, &speakers().id));
    assert!(!h.engine.in_users_use(&cable_in.id) && !h.engine.in_users_use(&a_in.id));
}

#[test]
fn calls_windows_left_on_a_cable_at_the_start_go_back_once_the_app_has_had_its_say() {
    // VB-Cable installed before CrossFX started, every default on it, the microphone never
    // picked. The speakers' lane holds its claim: calls in the cable are in CrossFX's path.
    let [cable_in, cable_out] = cable();
    let machine = vec![
        speakers(),
        fxsound(),
        microphone(),
        cable_in.clone(),
        cable_out.clone(),
    ];
    let mut h = harness_with(
        machine.clone(),
        all_roles(&cable_in.id),
        all_roles(&cable_out.id),
        &StartOptions::default(),
    );
    h.start();
    assert_eq!(
        h.defaults(OUT).get(Role::Communications),
        Some(cable_in.id.as_str()),
        "not before the app has said whether the settings know the cable"
    );
    h.forget();
    h.send(UiToAudio::SeedRememberedDefaults {
        output: speakers().id,
        input: microphone().id,
    });
    assert_eq!(
        h.did(),
        [Did::SetDefault(OUT, speakers().id, Role::Communications)]
    );
    assert_eq!(
        h.defaults(IN),
        all_roles(&cable_out.id),
        "the recording roles, which no lane holds, on a cable no lane uses, are left to Windows"
    );
    h.engine.shutdown();
    assert_eq!(h.defaults(OUT), all_roles(&speakers().id));

    // The app never speaks: the wait for it is up as the repair's is.
    let mut h = harness_with(
        machine.clone(),
        all_roles(&cable_in.id),
        all_roles(&cable_out.id),
        &StartOptions::default(),
    );
    h.start();
    h.forget();
    for _ in 0..=(REPAIR_PATIENCE.as_millis() / SUPERVISOR_PERIOD.as_millis()) {
        h.tick(SUPERVISOR_PERIOD);
    }
    assert_eq!(
        h.did(),
        [Did::SetDefault(OUT, speakers().id, Role::Communications)]
    );

    // The settings remember "CABLE Input" as the user's default from before CrossFX: theirs, and
    // their calls stay in it.
    let mut h = harness_with(
        machine,
        all_roles(&cable_in.id),
        all_roles(&cable_out.id),
        &StartOptions::default(),
    );
    h.start();
    h.send(UiToAudio::SeedRememberedDefaults {
        output: cable_in.id.clone(),
        input: String::new(),
    });
    h.tick(REPAIR_PATIENCE + SUPERVISOR_PERIOD);
    assert_eq!(
        h.defaults(OUT).get(Role::Communications),
        Some(cable_in.id.as_str())
    );
    h.engine.shutdown();
    assert_eq!(h.defaults(OUT), all_roles(&cable_in.id));
}

#[test]
fn a_claim_that_adopts_windows_roles_tells_the_settings_where_they_were() {
    // VB-Cable installed with the microphone's lane on: Windows gave the cable every recording
    // role, the lane adopts them on its cable and writes none — and the settings learn where they
    // were, for a run killed before the hand-back.
    let [cable_in, cable_out] = cable();
    let mut h = harness();
    h.start();
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    h.forget();
    windows_gives_a_new_cable_every_role(&mut h, &cable_in, &cable_out);
    assert!(
        !h.did()
            .iter()
            .any(|did| matches!(did, Did::SetDefault(IN, ..))),
        "adopted, not written: {:?}",
        h.did()
    );
    let messages = h.messages();
    assert!(
        messages.contains(&AudioToUi::RememberedDefault {
            direction: IN,
            node_name: microphone().id,
        }),
        "{messages:?}"
    );
    assert_eq!(h.engine.memory.input.most_recent_default, microphone().id);
    // Said once: a later run of the rules that learns nothing new says nothing.
    h.forget();
    h.engine.lanes.input.reclaim = true;
    h.event(Event::Devices);
    assert!(
        !h.messages()
            .iter()
            .any(|message| matches!(message, AudioToUi::RememberedDefault { .. }))
    );
}

#[test]
fn a_default_moved_onto_the_cable_the_microphone_stands_on_is_never_the_user_s() {
    // VB-Cable installed while CrossFX runs, its endpoints listed at once, the microphone's lane
    // taking it — and Windows' default for it heard only 2.5 s later, past PICK_WAIT. Whoever made
    // it the default, the cable is CrossFX's own now: whatever plays into it goes out with the
    // voice.
    let [cable_in, cable_out] = cable();
    let mut h = harness();
    h.start();
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    h.fake()
        .endpoints
        .extend([cable_in.clone(), cable_out.clone()]);
    h.event(Event::Devices);
    assert!(h.engine.lanes.input.claim.is_some());
    h.tick(Duration::from_millis(2500));
    h.forget();
    h.fake().defaults.output = all_roles(&cable_in.id);
    h.burst(Role::ALL.into_iter().map(|role| Event::DefaultChanged {
        direction: OUT,
        role,
        id: Some(cable_in.id.clone()),
    }));
    h.tick(SUPERVISOR_PERIOD);
    let messages = h.messages();
    assert!(!any_error_or_pick(&messages), "{messages:?}");
    assert_eq!(h.defaults(OUT), playback_on(&fxsound().id, &speakers().id));
    assert_eq!(input_claim(&h), Some(cable_out.id.clone()));
    assert!(!h.engine.in_users_use(&cable_in.id));
    h.engine.shutdown();
    assert_eq!(h.defaults(OUT), all_roles(&speakers().id));
}

#[test]
fn a_cable_the_user_made_their_default_while_crossfx_ran_stays_theirs_at_the_next_start() {
    // A ranking decides the speakers' device, as the app's settings have it by default. The
    // cable long installed, the user sends everything into "CABLE Input" for a streaming program:
    // theirs for the run, and — CrossFX's record keeping it — for the next.
    let [cable_in, cable_out] = cable();
    let mut h = harness_with(
        vec![
            speakers(),
            fxsound(),
            microphone(),
            cable_in.clone(),
            cable_out.clone(),
        ],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &both_ranked(),
    );
    h.start();
    h.tick(PICK_WAIT + SUPERVISOR_PERIOD);
    h.fake().defaults.output = all_roles(&cable_in.id);
    h.burst(Role::ALL.into_iter().map(|role| Event::DefaultChanged {
        direction: OUT,
        role,
        id: Some(cable_in.id.clone()),
    }));
    h.tick(SUPERVISOR_PERIOD);
    assert_eq!(h.fake().users_cables.output, all_roles(&cable_in.id));
    h.forget();
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    assert!(h.messages().contains(&AudioToUi::Error {
        direction: Some(IN),
        message: AudioError::VirtualInputDeviceInUse.to_string(),
    }));
    h.engine.shutdown();
    assert_eq!(h.defaults(OUT), all_roles(&cable_in.id), "left theirs");

    // The next start, on what that run left: the cable is theirs still.
    let mut h = harness_after(&mut h, &both_ranked());
    h.start();
    h.send(UiToAudio::SeedRememberedDefaults {
        output: speakers().id,
        input: microphone().id,
    });
    let messages = h.messages();
    assert!(
        messages.contains(&AudioToUi::RememberedDefault {
            direction: OUT,
            node_name: cable_in.id.clone(),
        }),
        "the default from before CrossFX, which the settings keep: {messages:?}"
    );
    assert_eq!(
        h.defaults(OUT),
        playback_on(&fxsound().id, &cable_in.id),
        "the playback roles CrossFX's, as at any start; the calls left in the cable"
    );
    h.forget();
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    assert!(h.messages().contains(&AudioToUi::Error {
        direction: Some(IN),
        message: AudioError::VirtualInputDeviceInUse.to_string(),
    }));
    h.engine.shutdown();
    assert_eq!(
        h.defaults(OUT),
        playback_on(&speakers().id, &cable_in.id),
        "the playback roles to the lane's own device, as for any default; calls theirs"
    );

    // And the one after: the record forgets the roles found elsewhere, and keeps the calls'.
    let mut h = harness_after(&mut h, &both_ranked());
    h.start();
    let mut calls = Defaults::default();
    calls.set(Role::Communications, Some(cable_in.id.clone()));
    assert_eq!(h.fake().users_cables.output, calls);
    h.tick(REPAIR_PATIENCE + SUPERVISOR_PERIOD);
    assert_eq!(
        h.defaults(OUT).get(Role::Communications),
        Some(cable_in.id.as_str())
    );
}

#[test]
fn a_lane_standing_on_a_cable_just_installed_plays_to_the_device_the_user_had() {
    // No FxSound driver: VB-Cable, installed while CrossFX runs, is the speakers' lane's device,
    // and takes the place of the default the user had on the headphones. That default is where
    // the lane plays, and where it is given back.
    let [cable_in, cable_out] = cable();
    let mut h = harness_with(
        vec![speakers(), headphones(), microphone()],
        all_roles(&headphones().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.start();
    windows_gives_a_new_cable_every_role(&mut h, &cable_in, &cable_out);
    assert_eq!(streams(&h), [(cable_in.id.clone(), headphones().id)]);
    assert!(h.messages().contains(&AudioToUi::RememberedDefault {
        direction: OUT,
        node_name: headphones().id,
    }));
    assert_eq!(
        h.defaults(IN),
        all_roles(&microphone().id),
        "the recorders off the cable's recording side, which records the desktop"
    );
    h.engine.shutdown();
    assert_eq!(h.defaults(OUT), all_roles(&headphones().id));
}

#[test]
fn a_lane_ending_leaves_the_other_lane_s_defaults_to_its_claim() {
    // The speakers' lane plays on FxSound's driver and the microphone's waits on VB-Cable; Cable A
    // has the playback roles, unheard yet, when the microphone's lane is switched off. Its
    // hand-back puts Windows' defaults in CrossFX's path right — the speakers' roles back on
    // FxSound's driver, which still plays, not straight to the speakers past it.
    let [cable_in, cable_out] = cable();
    let [a_in, a_out] = cable_a();
    let mut h = harness_with(
        vec![
            speakers(),
            fxsound(),
            microphone(),
            cable_in,
            cable_out.clone(),
            a_in.clone(),
            a_out,
        ],
        all_roles(&speakers().id),
        all_roles(&microphone().id),
        &StartOptions::default(),
    );
    h.start();
    h.send(UiToAudio::SelectDevice {
        node_name: microphone().id,
        direction: IN,
    });
    assert_eq!(input_claim(&h), Some(cable_out.id.clone()));
    h.fake().defaults.output = all_roles(&a_in.id);
    h.forget();
    h.send(UiToAudio::DetachLane(IN));
    assert_eq!(h.defaults(OUT), playback_on(&fxsound().id, &speakers().id));
    assert_eq!(h.defaults(IN), all_roles(&microphone().id));
    assert_eq!(streams(&h), [(fxsound().id, speakers().id)]);
}
