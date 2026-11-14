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
    /// The record of the defaults FxSound holds, as kept: the output lane's, the input lane's.
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
    /// FxSound's record of the defaults it holds (`record`), as the last run left it.
    record: PerDirection<Option<String>>,
    log: Arc<std::sync::Mutex<Vec<Did>>>,
    /// The running streams' "ended by themselves" switches, by lane.
    ended: PerDirection<Option<EndSwitch>>,
    /// Set, the next stream asked to stop sticks in WASAPI: its thread keeps the lane's DSP until
    /// the sender of this is dropped.
    stuck: Arc<std::sync::Mutex<Option<Receiver<()>>>>,
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
        self.original.clone()
    }

    fn claimed(&mut self) -> PerDirection<Option<String>> {
        self.record.clone()
    }

    fn keep_claimed(&mut self, claimed: &PerDirection<Option<String>>) {
        self.record = claimed.clone();
        self.did(Did::Recorded(claimed.output.clone(), claimed.input.clone()));
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

/// [`harness_with`], on a machine whose record of FxSound's defaults (`record`) is `record`: what a
/// run before this one left.
fn harness_recorded(
    endpoints: Vec<Endpoint>,
    output_defaults: Defaults,
    input_defaults: Defaults,
    options: &StartOptions,
    record: PerDirection<Option<String>>,
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
        log: Arc::clone(&log),
        ended: PerDirection::default(),
        stuck: Arc::default(),
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

/// The machine this port is written on: the Realtek speakers and microphone array, and FxSound's
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
        self.engine.event(event);
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
    let did = h.did();
    assert_eq!(
        did[0],
        Did::Started(IN, microphone().id, cable_in.id.clone())
    );
    assert_eq!(did[1..], claimed(IN, &cable_out.id));
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

#[test]
fn while_the_original_fxsound_runs_the_output_lane_stands_aside_and_says_why() {
    let mut h = harness();
    h.fake().original = Some(r"C:\Program Files\FxSound LLC\FxSound\FxSound.exe".to_owned());
    h.start();
    assert_eq!(h.did(), [], "neither a default nor a stream");
    let messages = h.messages();
    assert!(messages.contains(&AudioToUi::Warning {
        direction: Some(OUT),
        message: fxsound_core::i18n::tr(ORIGINAL_FXSOUND_RUNNING),
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
fn the_original_starting_while_fxsound_holds_the_default_is_left_the_default_without_a_fight() {
    let mut h = harness();
    h.start();
    h.forget();
    h.fake().original = Some("FxSound.exe (process 12364)".to_owned());
    h.tick(ORIGINAL_EVERY);
    assert_eq!(
        h.did(),
        [Did::Stopped(OUT, Duration::ZERO)],
        "stopped, and no default written: it is the original's now"
    );
    assert!(h.messages().contains(&AudioToUi::Warning {
        direction: Some(OUT),
        message: fxsound_core::i18n::tr(ORIGINAL_FXSOUND_RUNNING),
    }));
    // And nothing is handed back at the exit either.
    h.forget();
    h.engine.shutdown();
    assert_eq!(h.did(), []);
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
    // FxSound's own writes come back first, and are no pick.
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

/// A headset's microphone, for calls: what a user may record calls from instead of FxSound.
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

#[test]
fn calls_the_user_moves_while_fxsound_holds_them_stay_where_they_were_put() {
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
fn the_playback_roles_go_back_to_the_device_the_user_picked_in_fxsound() {
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
        "where the user listened through FxSound, as the original gives it back"
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
    // detached, as it is at every start until a microphone is picked.
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
    // The power is off, and the original holds the default when FxSound starts.
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
    h.send(UiToAudio::KeepInputAwake(true));
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
            message: fxsound_core::i18n::tr(ONE_HEADSET_ON_BOTH_LANES),
        }]
    );
}

#[test]
fn a_new_voice_chain_is_built_in_place_while_the_lane_is_off_and_handed_over_while_it_runs() {
    let [cable_in, cable_out] = cable();
    let mut h = harness_with(
        vec![speakers(), fxsound(), microphone(), cable_in, cable_out],
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
    assert!(h.engine.lanes.input.stream.is_some());
    h.send(UiToAudio::SetInputChain("broadcast".to_owned()));
    assert!(
        h.engine.handover.replacement.is_full(),
        "built here and sent to the stream"
    );
    assert!(!h.engine.input_spec_pending);
}
