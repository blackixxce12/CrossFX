//! The CrossFX audio backend for Linux: a PipeWire virtual sink in front of the speakers and a
//! PipeWire virtual source behind the microphone, each with its own DSP chain in its process
//! callback, running side by side.
//!
//! # What this replaces
//!
//! On Windows, the original FxSound ships a kernel virtual audio driver, forces it to be the system
//! default render endpoint, captures from it with `AUDCLNT_STREAMFLAGS_LOOPBACK`, runs the DSP over
//! the captured buffer and renders the result to the real device — the `audiopassthru/` module,
//! reverse-engineered in full in `docs/spec/12-audio-io.md`. On Linux none of that machinery is
//! needed: a `pw_stream` with `media.class = "Audio/Sink"` **is** a sink, so the audio applications
//! write arrives directly in our own process callback. There is no driver to install, no reboot, no
//! root, no loopback capture and no monitor hop.
//!
//! # Windows
//!
//! On Windows CrossFX does what FxSound for Windows does, with this crate's `wasapi` backend in
//! PipeWire's place: a virtual playback device the user has installed — FxSound's own driver
//! ("FxSound Speakers") or a virtual cable; CrossFX ships no driver — is made the default,
//! captured with WASAPI loopback, run through the same lane DSP and played to the real device; and
//! behind the microphone, the microphone is processed and played into a cable of its own, whose
//! recording side becomes the default — opened, as the Linux lane's is, only while something
//! records from it. Everything below that names PipeWire is Linux's. The public
//! API, the lanes' DSP, the device rules and the paths to the GUI are the same code on both, and
//! `wasapi`'s module docs say what each control message means there.
//!
//! # Topology
//!
//! One pair of nodes per **lane** — one lane per [`crossfx_core::DeviceDirection`] — the two nodes
//! of a pair sharing a `node.link-group` of their own ([`link_group`]): the design
//! `docs/spec/12-audio-io.md` §18 chose (Option A), §19.1 draws, and §29 extends to two lanes.
//! Either lane can be enabled or detached on its own, and both pairs can exist at once
//! (`docs/0.4.0-design.md` §1). The two lanes are mirror images of each other, and the two process
//! callbacks and the ring between them are the same code in both — each lane has its own ring,
//! its own chain and its own paths to the GUI:
//!
//! ```text
//!  OUTPUT LANE (the original's behaviour; enabled at start):
//!
//!  apps ──► crossfx_sink    (Audio/Sink, Direction::Input)    process(): DSP in place ──┐
//!                                                                                       │ ring
//!           crossfx_output  (Stream/Output/Audio, Output)     process(): pop ◄──────────┘
//!                │
//!                ▼  target.object = <chosen sink's node.name>
//!           alsa_output.…   (the user's real speakers)
//!
//!  INPUT LANE (Linux only, `docs/spec/12-audio-io.md` §28; enabled when a microphone is picked):
//!
//!           alsa_input.…    (the user's real microphone)
//!                │
//!                ▼  target.object = <chosen source's node.name>
//!           crossfx_capture (Stream/Input/Audio, Direction::Input)  process(): DSP in place ──┐
//!                                                                                            │ ring
//!  apps ◄── crossfx_source  (Audio/Source, Direction::Output)       process(): pop ◄─────────┘
//! ```
//!
//! Why two streams rather than one filter: `pipewire` 0.10.1 has no `pw_filter` binding at all
//! (`docs/api/pipewire-0.10-rust.md` §12), and even if it had, one node cannot be both an
//! `Audio/Sink` and a stream that targets a chosen device. Why not a `module-null-sink` created
//! through `pactl`: a null sink outlives a `SIGKILL`ed app, and if it was the default the user is
//! left with a silent machine — the exact failure class the project `CLAUDE.md` warns about for
//! `audiopassthru/`. Nodes owned by our own client connection disappear the instant the socket
//! closes.
//!
//! The `node.link-group` is not decoration: it is how WirePlumber learns our nodes are one
//! logical device and refuses to link `crossfx_output` back into `crossfx_sink` — or, in the input
//! lane, `crossfx_capture` into `crossfx_source` — which is otherwise exactly what happens the
//! moment our node becomes the default.
//!
//! It is not only WirePlumber's business, either. Since 0.3.68 the PipeWire server schedules the
//! members of a link-group together: a node made runnable by a link of its own makes every other
//! member runnable too (`run_nodes` in `src/pipewire/context.c`). That is what lets the output
//! lane's playback stream sleep while nothing plays into the sink, and the input lane's capture
//! stream — and the microphone with it — while nothing records from the source (see `engine`,
//! "Idle"), and it is why each lane has a group **of its own** — `crossfx` for the speakers,
//! `crossfx-input` for the microphone (`docs/spec/12-audio-io.md` §29.2). WirePlumber would be
//! content either way: each stream still shares a group with its own virtual node, which is all
//! its refusal needs, and no link either lane needs ever joins two of our own nodes. The server is
//! not. With one group, a capture stream fed by a microphone made the speakers' pair runnable with
//! it, and the speakers never went idle again while the microphone lane ran.
//!
//! With both lanes enabled the system sees *two* CrossFX devices: "CrossFX (Output)" under its
//! sinks and "CrossFX (Input)" under its sources, each with the word in the system language
//! ([`locale`]). Detaching a lane takes only its own device away.
//!
//! # Echo cancellation
//!
//! Off by default, and on request the input lane records through PipeWire's own canceller rather
//! than from the microphone directly (`docs/0.4.0-design.md` §7). The canceller is
//! `libpipewire-module-echo-cancel`, loaded into this crate's PipeWire context (`aec`): it hears
//! the microphone and the monitor of the speakers the output lane plays to, and its
//! `crossfx_aec_source` takes the microphone's place in front of `crossfx_capture`. The three nodes
//! it makes are CrossFX's as much as the lanes' four are — named by us, in a link-group of their
//! own ([`AEC_LINK_GROUP`]), never offered as devices — and they come and go with the microphone
//! lane: loaded once it has a pair, reloaded when its microphone or the speakers change, unloaded
//! when echo cancellation is switched off, when the lane is detached, and before the connection
//! or the thread ends.
//!
//! # Threads
//!
//! Nothing in `pipewire` or `libspa` is `Send`, so the whole PipeWire side lives on one thread
//! that this crate spawns and owns. The GUI keeps an [`EngineHandle`], which is `Send` and talks
//! over paths chosen for what each one carries. Every path that reaches a chain is **per lane** —
//! the output lane's music chain and the input lane's voice chain each have their own — so an
//! event meant for the microphone can never be consumed by the music chain, and the two chains'
//! meters can never overwrite each other:
//!
//! | Direction | Carries | Mechanism | Per lane | Why |
//! | --- | --- | --- | --- | --- |
//! | GUI → DSP | [`DspParams`] / [`InputDspParams`] snapshots | [`triple_buffer`] | one per chain: [`EngineHandle::set_params`], [`EngineHandle::set_input_params`] | Wait-free on both ends, no lock, no allocation. A coalesced intermediate snapshot is harmless: the next one supersedes it. |
//! | GUI → DSP | [`DspEvent`] one-shots | bounded `crossbeam_channel` | one per lane: [`EngineHandle::send_event`] | Must not be coalesced. Pre-allocated array channel; `try_recv` never allocates and never parks. |
//! | DSP → GUI | [`Meters`] | [`triple_buffer`] | one per lane: [`EngineHandle::meters`] | Same reasoning, other way round. |
//! | GUI ↔ control | [`UiToAudio`] / [`AudioToUi`] | `pipewire::channel` / `crossbeam_channel` | shared; messages name their lane | May allocate and block; never touched from the process callback. |
//!
//! The process callback itself allocates nothing, locks nothing and cannot panic: every buffer it
//! needs is sized once in [`AudioEngine::start`] for the worst case in §24 of the spec (2048
//! frames × 8 channels), every `Option` is handled with `let … else { return }`, and the two
//! callbacks exchange samples through an SPSC ring built out of atomics.
//!
//! # The session default
//!
//! Once a lane's pair of nodes is up, CrossFX **does** make itself the session default for that
//! lane's direction — `default.configured.audio.sink = crossfx_sink` for the output lane,
//! `default.configured.audio.source = crossfx_source` for the input lane — because that is the
//! whole point: the user picks *their* speakers or *their* microphone in CrossFX and every
//! application follows without being re-routed by hand, exactly as the original's driver does. Each
//! lane holds its own claim. It does so politely, per `docs/spec/12-audio-io.md` §21: the default
//! that was there before is remembered first and handed back — on exit (both lanes'), when a lane
//! is detached (that lane's), and whenever a lane's nodes go away for good — **before** the nodes
//! are destroyed, so there is never a window in which a default names a node that no longer
//! exists. Only the `configured` keys are ever written; `default.audio.*` is WirePlumber's. A
//! caller can opt out, lane by lane, with [`UiToAudio::SetAsDefault`] with `want: false`.
//!
//! # What it deliberately does not do
//!
//! * **It never writes volume or mute on the target device.** In the original FxSound for Windows
//!   the DFX endpoint and the real endpoint are two separate system volume controls that it mirrors
//!   (`sndDevicesVolCallbacks.cpp`). On Linux, once `crossfx_sink` is the default it *is* the
//!   control the volume keys, `wpctl` and every desktop slider target; mirroring would fight
//!   WirePlumber's own restore and would leave the real device permanently changed after we exit.
//!   The whole `savedPlaybackVolume` / `b_never_raise_volume` contract therefore disappears — see
//!   `docs/spec/12-audio-io.md` §19.7. What it keeps instead is CrossFX's *own* volume per target
//!   device, written only to its own two virtual nodes, applied after the chain, and never raised
//!   by a change of device (`volume`, §19.7.1).
//! * **It never resamples.** The zero-order-hold upsampler at `sndDevicesDoPlayback.cpp:79-89` is
//!   not ported; PipeWire's sinc resampler handles any rate mismatch — and its channel mixer turns
//!   a mono microphone into the stereo pair the DSP runs on, and that pair into a mono headset.
//! * **It never wins the default implicitly.** `priority.session` is deliberately low, so a
//!   WirePlumber that has never been told a default picks real hardware, not us; the default is
//!   taken only through the explicit metadata write above, which is also the only thing that can
//!   be handed back.
//! * **It never feeds one lane from the other.** The two lanes share nothing, not even a
//!   link-group: each has its own group, its own ring, its own chain, its own counters and its own
//!   claim on a default, so the music chain cannot reach the microphone's signal or the other way
//!   round, the microphone running cannot keep the speakers awake, and one lane failing leaves the
//!   other playing. Echo cancellation is the one bridge, and only while it is on: the canceller
//!   hears what the speakers play in order to take its echo out of the microphone — nothing of it
//!   reaches a recording — and it hears that on the microphone's clock, so the speakers and the
//!   output pair run for as long as it does (`docs/0.4.0-design.md` §7).
//!
//! [`DspParams`]: crossfx_core::messages::DspParams
//! [`InputDspParams`]: crossfx_core::messages::InputDspParams
//! [`DspEvent`]: crossfx_core::messages::DspEvent
//! [`Meters`]: crossfx_core::messages::Meters
//! [`UiToAudio`]: crossfx_core::messages::UiToAudio
//! [`AudioToUi`]: crossfx_core::messages::AudioToUi
//! [`UiToAudio::SetAsDefault`]: crossfx_core::messages::UiToAudio::SetAsDefault

// `deny`, not `forbid`, for exactly two reasons, one per backend. On Linux, the echo canceller:
// PipeWire's canceller is a module (`libpipewire-module-echo-cancel`, `docs/0.4.0-design.md` §7),
// and `pipewire` 0.10.1 has no binding for loading one — no `pw_context_load_module`, no
// `pw_impl_module` at all. So `aec` calls the C functions through `pipewire-sys`, and those three
// functions carry an `#[allow(unsafe_code)]`. On Windows, COM: every call into WASAPI, the
// MMDevice API and the default-device policy is `unsafe`, and all of them are in one module,
// `wasapi::com`, behind safe wrappers — the one module of the Windows backend allowed it. Each
// `unsafe` block in either says beside it why it is sound. Everything else here, the process
// callbacks and the stream threads above all, stays as safe as `forbid` made it, and a new
// `unsafe` anywhere else still fails the build.
#![deny(unsafe_code)]

// Two backends behind one API. Everything PipeWire is Linux's: the nodes, the graph, the echo
// canceller, the per-application routes, the handover and WirePlumber's volume. Windows has its
// own (`wasapi`), the way FxSound for Windows itself works: the system default moved to a virtual
// playback device, captured with WASAPI loopback, processed and played to the real speakers. The
// DSP of a lane (`lane_dsp`), the device rules (`devices`), the paths to the GUI and the public
// types are the same code on both.
#[cfg(not(any(target_os = "linux", windows)))]
compile_error!("CrossFX builds for Linux (PipeWire) and Windows (WASAPI)");

#[cfg(target_os = "linux")]
mod aec;
#[cfg(target_os = "linux")]
mod app_routes;
#[cfg(target_os = "linux")]
mod app_streams;
pub mod devices;
#[cfg(target_os = "linux")]
pub mod engine;
mod lane_dsp;
pub mod locale;
mod per_direction;
#[cfg(target_os = "linux")]
mod routes;
mod spa_channels;
#[cfg(target_os = "linux")]
mod stranded;
#[cfg(target_os = "linux")]
mod stream_handover;
#[cfg(target_os = "linux")]
mod volume;
#[cfg(windows)]
mod wasapi;
pub mod wireplumber_hook;

use std::thread::JoinHandle;
use std::time::Duration;

#[cfg(target_os = "linux")]
use crossbeam_channel::RecvTimeoutError;
use crossbeam_channel::{Receiver, Sender, TrySendError};
use crossfx_core::DeviceDirection;
use crossfx_core::messages::{
    AudioToUi, DspEvent, DspParams, InputDspParams, Meters, TargetVolume, UiToAudio,
};
#[cfg(target_os = "linux")]
use triple_buffer::TripleBuffer;
use triple_buffer::{Input, Output};

use crate::per_direction::PerDirection;

pub use devices::{
    BLUEZ_HEADSET_RATE, ChannelMap, DeviceInfo, FormFactor, MAX_CHANNELS, MAX_SAMPLE_RATE,
    MIN_CHANNELS, Preference, Selection, SelectionMemory,
};
pub use locale::{node_description, sink_description, source_description};
/// What the Windows engine sees, as a report for a person to read: every endpoint as the engine
/// classifies it, the virtual device each lane would stand on, the formats, the defaults and the
/// original FxSound. Read only: nothing is changed (`wasapi::probe`).
#[cfg(windows)]
pub use wasapi::probe as wasapi_probe;

/// What the `node.name` of every node of this crate's starts with: the lanes' pairs, the node that
/// holds the microphone awake, the echo canceller's and the per-application routes'
/// ([`OUR_NODE_NAMES`], [`AEC_NODE_PREFIX`], [`ROUTE_NODE_PREFIX`]). The WirePlumber hook's script
/// tells CrossFX's own nodes by it too (`wireplumber_hook`).
pub const NODE_PREFIX: &str = "crossfx_";

/// `node.name` of the virtual sink applications render into (output direction, NODE 1).
///
/// This string is CrossFX's analogue of the original's `SND_DEVICES_DFX_DEVICE_STRING`
/// (`audiopassthru/include/sndDevices.h:51`) and, like it, is matched by name elsewhere — it is
/// what gets written into `default.configured.audio.sink`, so it must stay stable across releases.
pub const SINK_NODE_NAME: &str = "crossfx_sink";

/// `node.name` of the stream that renders into the user's real device (output direction, NODE 2).
pub const OUTPUT_NODE_NAME: &str = "crossfx_output";

/// `node.name` of the stream that captures from the user's real microphone (input direction,
/// NODE 1).
pub const CAPTURE_NODE_NAME: &str = "crossfx_capture";

/// `node.name` of the virtual source applications record from (input direction, NODE 2).
///
/// Written into `default.configured.audio.source`; stable for the same reason as
/// [`SINK_NODE_NAME`].
pub const SOURCE_NODE_NAME: &str = "crossfx_source";

/// `application.id` of every node CrossFX makes: its application id
/// ([`crossfx_core::identity::APP_ID`]), which desktops find its icon and its name by.
#[cfg(target_os = "linux")]
pub(crate) const APP_ID: &str = crossfx_core::identity::APP_ID;

/// `node.name` of the echo canceller's capture stream: the one that hears the microphone
/// (`docs/0.4.0-design.md` §7).
pub const AEC_CAPTURE_NODE_NAME: &str = "crossfx_aec_capture";

/// `node.name` of the echo canceller's monitor stream: the one that hears what the speakers play,
/// the echo it takes out of the microphone.
pub const AEC_MONITOR_NODE_NAME: &str = "crossfx_aec_monitor";

/// `node.name` of the echo canceller's source: the microphone with the echo taken out, and what the
/// input lane's capture stream records from while echo cancellation runs.
pub const AEC_SOURCE_NODE_NAME: &str = "crossfx_aec_source";

/// `node.name` of the stream CrossFX records its own virtual source with while the app holds the
/// microphone awake ([`UiToAudio::KeepInputAwake`], `docs/0.4.0-upstream.md` U19): the calibration
/// wizard and the microphone meters.
///
/// The input lane's capture stream is passive, so the microphone runs only while something records
/// from CrossFX (Input); this is that something when nobody else is. It is an ordinary recording
/// stream with no `node.link-group` — the kind WirePlumber 0.5 switches a Bluetooth headset to its
/// call profile for (`device/autoswitch-bluetooth-profile.lua`) — so it wakes a headset's
/// microphone exactly as a call would.
///
/// [`UiToAudio::KeepInputAwake`]: crossfx_core::messages::UiToAudio::KeepInputAwake
pub const KEEP_AWAKE_NODE_NAME: &str = "crossfx_mic_check";

/// The four nodes of the two lanes' pairs, in lane order: the output lane's, then the input lane's.
pub const LANE_NODE_NAMES: [&str; 4] = [
    SINK_NODE_NAME,
    OUTPUT_NODE_NAME,
    CAPTURE_NODE_NAME,
    SOURCE_NODE_NAME,
];

/// The three nodes the echo canceller's module makes while echo cancellation runs.
pub const AEC_NODE_NAMES: [&str; 3] = [
    AEC_CAPTURE_NODE_NAME,
    AEC_MONITOR_NODE_NAME,
    AEC_SOURCE_NODE_NAME,
];

/// Every node this crate ever creates, or has PipeWire create for it: [`LANE_NODE_NAMES`], then
/// [`AEC_NODE_NAMES`], then [`KEEP_AWAKE_NODE_NAME`]. None of them is a device CrossFX could attach
/// to, so [`DeviceInfo::from_props`] drops them by name whatever their `media.class` says. The
/// canceller's source is an `Audio/Source` like any microphone, and offered as one it would let the
/// input lane capture from its own canceller.
pub const OUR_NODE_NAMES: [&str; 8] = [
    SINK_NODE_NAME,
    OUTPUT_NODE_NAME,
    CAPTURE_NODE_NAME,
    SOURCE_NODE_NAME,
    AEC_CAPTURE_NODE_NAME,
    AEC_MONITOR_NODE_NAME,
    AEC_SOURCE_NODE_NAME,
    KEEP_AWAKE_NODE_NAME,
];

/// What the `node.name` of every node of a per-application route starts with
/// (`docs/0.4.0-apps.md`): `crossfx_route_o<N>` and `crossfx_route_o<N>_play` in front of the
/// speakers, `crossfx_route_i<N>_capture` and `crossfx_route_i<N>` behind the microphone.
///
/// A prefix rather than a list, because how many routes there are is the user's rules' business,
/// and every one of them is CrossFX's as much as the lanes' own pairs are ([`is_crossfx_node`]).
pub const ROUTE_NODE_PREFIX: &str = "crossfx_route_";

/// What the `node.name` of every node of the echo canceller starts with ([`AEC_NODE_NAMES`]).
///
/// The three names are listed too; the prefix is what [`is_crossfx_node`] matches, so a node the
/// module adds in some later version, named the way the others are, is ours from its first day.
pub const AEC_NODE_PREFIX: &str = "crossfx_aec_";

/// Whether the node called `node_name` is one of CrossFX's own: a lane's pair, the node that
/// keeps the microphone awake ([`OUR_NODE_NAMES`]), the echo canceller's ([`AEC_NODE_PREFIX`]),
/// or a per-application route's ([`ROUTE_NODE_PREFIX`]).
///
/// None of them is a device to attach to, and none is an application's stream: an application
/// list that offered CrossFX's own playback stream a preset would offer to run CrossFX through
/// itself.
#[must_use]
pub fn is_crossfx_node(node_name: &str) -> bool {
    OUR_NODE_NAMES.contains(&node_name)
        || node_name.starts_with(ROUTE_NODE_PREFIX)
        || node_name.starts_with(AEC_NODE_PREFIX)
}

/// The `node.link-group` of the output lane's two nodes. **Mandatory**; see the module docs.
pub const LINK_GROUP: &str = "crossfx";

/// The `node.link-group` of the input lane's two nodes: a group of their own, not the output
/// lane's, because the server runs a group's members together (module docs).
pub const INPUT_LINK_GROUP: &str = "crossfx-input";

/// The `node.link-group` of the echo canceller's three streams: a group of their own, neither
/// lane's (`docs/0.4.0-design.md` §7, and `aec` for the reasoning).
///
/// Not the input lane's, because the input lane's capture stream links to the canceller's source,
/// and WirePlumber refuses a link between two nodes of one group. Not the output lane's, because
/// the server runs a group together, and the canceller's capture stream — which runs whenever the
/// microphone does — would keep the speakers' pair running with it.
pub const AEC_LINK_GROUP: &str = "crossfx-aec";

/// The `node.link-group` of a lane's pair.
#[must_use]
pub const fn link_group(direction: DeviceDirection) -> &'static str {
    match direction {
        DeviceDirection::Output => LINK_GROUP,
        DeviceDirection::Input => INPUT_LINK_GROUP,
    }
}

/// The product name every description starts with, and the `media.name` of the two streams:
/// [`crossfx_core::identity::PRODUCT`].
///
/// The virtual sink and source are *not* described with this string alone: their
/// `node.description` is `"CrossFX (<Output|Input>)"` in the system language — see
/// [`sink_description`] / [`source_description`] — so the two can be told apart in a device list.
pub const SINK_DESCRIPTION: &str = crossfx_core::identity::PRODUCT;

/// `node.description` of the playback stream (output direction, NODE 2). Not localised: it is an
/// internal node UIs group under the sink through the link-group.
pub const OUTPUT_STREAM_DESCRIPTION: &str = "CrossFX output";

/// `node.description` of the capture stream (input direction, NODE 1). Not localised, as above.
pub const CAPTURE_STREAM_DESCRIPTION: &str = "CrossFX capture";

/// `node.description` and `media.name` of the stream that holds the microphone awake
/// ([`KEEP_AWAKE_NODE_NAME`]). Not localised either, but — unlike the pair's two streams — seen: a
/// desktop lists it among the applications recording, as "CrossFX" with this beside it, for as
/// long as the calibration wizard or the microphone meters are open.
pub const KEEP_AWAKE_STREAM_DESCRIPTION: &str = "CrossFX microphone check";

/// What the engine tells the user, translated, when one Bluetooth headset is the target of both
/// lanes ([`AudioToUi::Warning`], `docs/0.4.0-upstream.md` U9): the headset runs its call profile
/// while anything records from it through CrossFX, and the music lane's sink goes with it — one
/// channel, 8 to 32 kHz depending on the codec, 16 kHz on nearly every headset.
///
/// The English text is the key [`crossfx_core::i18n::tr`] looks the translation up by. Public so
/// the app's translation tables and their audit can name it: the audit reads the crates that draw
/// text, and this string reaches the screen from here.
pub const ONE_HEADSET_ON_BOTH_LANES: &str =
    "Using this headset's microphone switches it to call quality: music plays in mono at 16 kHz";

/// What the engine tells the user, translated, when an application's preset cannot get a route of
/// its own ([`AudioToUi::Warning`], `docs/0.4.0-apps.md`): its lane runs as many routes as it may
/// already, every one of them in use, so the application plays — or records — through the lane's
/// own preset. The first `%s` is the application's name as the window shows it, the second the
/// most presets a lane runs for applications at once ([`crossfx_core::MAX_ROUTES_PER_LANE`]).
///
/// Public for the reason [`ONE_HEADSET_ON_BOTH_LANES`] is: the app's translation tables and their
/// audit name it, and this string reaches the screen from here.
pub const TOO_MANY_APPLICATION_PRESETS: &str =
    "%s stays on CrossFX's preset: at most %s application presets can run at once";

/// What the Windows engine tells the user, translated, while the original FxSound — FxSound LLC's,
/// under `…\FxSound LLC\FxSound\` — runs ([`AudioToUi::Warning`] for the output lane): it holds the
/// same virtual device, the one it makes every application play into, so the output lane neither
/// takes the default nor processes until it has quit. Two programs capturing one device and
/// playing it to the speakers would play every sound twice, and each would take the default back
/// from the other.
///
/// Public for the reason [`ONE_HEADSET_ON_BOTH_LANES`] is: the app's translation tables and their
/// audit name it, and this string reaches the screen from here. Defined on every platform so that
/// the audit reads it wherever it runs; only the Windows engine sends it.
pub const ORIGINAL_FXSOUND_RUNNING: &str = "The original FxSound is running and is using the \
                                            virtual device: quit it so that CrossFX can process \
                                            the sound";

/// The `node.name` of CrossFX's virtual device for a direction — the value written into that
/// direction's `default.configured.audio.*` key.
#[must_use]
pub const fn our_node_name(direction: DeviceDirection) -> &'static str {
    match direction {
        DeviceDirection::Output => SINK_NODE_NAME,
        DeviceDirection::Input => SOURCE_NODE_NAME,
    }
}

/// The largest quantum the process callbacks are sized for, in frames
/// (`docs/spec/12-audio-io.md` §24). A larger graph quantum is processed in several passes rather
/// than being truncated.
pub const MAX_QUANTUM_FRAMES: usize = 2048;

/// How many channels a lane's gains are kept for: every channel CrossFX will ever run
/// ([`MAX_CHANNELS`]). The size of `lane_dsp`'s per-channel arrays on both backends.
pub(crate) const LANE_CHANNELS: usize = MAX_CHANNELS as usize;

/// The quantum CrossFX asks for by default: 512 frames, ≈10.7 ms at 48 kHz, the `Normal` row of
/// the spec's latency table.
pub const DEFAULT_QUANTUM_FRAMES: u32 = 512;

/// Sample rate used until the server tells us otherwise.
pub const DEFAULT_SAMPLE_RATE: u32 = 48_000;

/// Ring capacity in frames, `8 × MAX_QUANTUM_FRAMES` — 512 KiB at 8 channels
/// (`docs/spec/12-audio-io.md` §24).
pub const RING_CAPACITY_FRAMES: usize = 8 * MAX_QUANTUM_FRAMES;

/// How many [`DspEvent`]s may be in flight before the oldest is dropped.
pub const EVENT_QUEUE_LEN: usize = 64;

/// How long [`AudioEngine::start`] waits for the *first* connection attempt before handing the
/// caller a handle anyway and letting the engine keep retrying in the background.
///
/// Connecting to a socket that is not there fails in microseconds, so in the failure case this
/// timeout is never approached; it only bounds the pathological "server accepted the socket but
/// never replied" case.
pub const START_TIMEOUT: Duration = Duration::from_millis(1500);

/// Everything that can go wrong, in 1:1 correspondence with the states the original's GUI already
/// knows how to render.
///
/// The comments give the `sndDevices.h:74-143` code each variant replaces, so
/// `FxController`'s existing error branches port without re-deriving the mapping.
///
/// Two of those states have no variant: `-57 SND_DEVICES_NO_VALID_PLAYBACK_DEVICE` ("no stereo
/// output") and `-58 SND_DEVICES_ASK_USER_SELECT_PLAYBACK_DEVICE` ("the chosen output is mono,
/// pick another"). Both existed only to refuse mono playback devices, a workaround for a Windows
/// driver bug (`sndDevices.h:32-39`); PipeWire drives a mono device perfectly well, so CrossFX
/// attaches to one instead (`docs/spec/12-audio-io.md`, open question 6).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AudioError {
    /// No `Audio/Sink` node exists other than ours. ≡ `209 SND_DEVICES_NO_REAL_DEVICES_FOUND`.
    #[error("no output devices present")]
    NoOutputDevices,
    /// No `Audio/Source` node exists other than ours. The input-direction twin of
    /// [`AudioError::NoOutputDevices`]; the original FxSound for Windows had no capture mode and so
    /// no code for it.
    #[error("no input devices present")]
    NoInputDevices,
    /// The requested device is not in the graph. ≡ `-2 SND_DEVICES_DEVICE_NOT_PRESENT`.
    #[error("selected device is not present")]
    DeviceNotPresent,
    /// The device is present but refused the stream. ≡ `-54` plus `playbackDeviceIsUnavailable`.
    #[error("audio device is unavailable")]
    DeviceUnavailable,
    /// PipeWire could not be reached at all — no socket, no `XDG_RUNTIME_DIR`, or the connection
    /// was refused. The original has no analogue: its driver was always present.
    #[error("PipeWire is not available: {0}")]
    PipewireUnavailable(String),
    /// The connection dropped after having worked. The engine is already retrying.
    #[error("lost connection to PipeWire")]
    PipewireDisconnected,
    /// The server would not agree a format we can process. ≡ `-35` / `-36`.
    #[error("format negotiation failed")]
    FormatNegotiation,
    /// Windows only: no virtual playback device to stand in front of the speakers. The original
    /// FxSound for Windows installs its own kernel driver for that ("FxSound Audio Enhancer");
    /// CrossFX ships no driver, and uses the FxSound driver or a virtual cable the user installed
    /// (`docs/1.0.0-windows-port.md`). ≡ the original's "audio device is missing" state.
    #[error("no virtual playback device is installed")]
    NoVirtualDevice,
    /// Windows only: no virtual cable left for the input lane — none is installed, or the only one
    /// is the output lane's, as on a machine without FxSound's driver, where the speakers' lane
    /// stands on VB-CABLE. The input lane plays the processed microphone into a cable's playback
    /// side, for applications to record from its recording side, and needs a cable of its own:
    /// FxSound's driver records nothing it plays, and the output lane's cable carries the
    /// speakers' sound. So the user installs VB-CABLE, or a second cable where the output lane has
    /// the first. The input twin of [`AudioError::NoVirtualDevice`]. A cable that is there but in
    /// the user's own use is [`AudioError::VirtualInputDeviceInUse`].
    #[error("no virtual cable of its own for the microphone")]
    NoVirtualInputDevice,
    /// Windows only: a virtual cable is there for the input lane, but the user has it in use — they
    /// picked it as the output lane's device, which plays into it, or a default device of theirs
    /// is on one of its sides — and the lane does not mix the microphone into what they send
    /// through it. The user frees it by picking another output device in CrossFX, or making another
    /// device the default, or installs a second cable for the microphone. A default Windows put on
    /// the cable by itself, as it was installed, is no use of theirs: the lane takes the cable then.
    #[error("the virtual cable for the microphone is in use")]
    VirtualInputDeviceInUse,
    /// Windows only: the Windows audio service could not be reached — COM, the MMDevice
    /// enumerator or an `IAudioClient` failed outright. The Windows twin of
    /// [`AudioError::PipewireUnavailable`].
    #[error("Windows audio is not available: {0}")]
    WindowsAudioUnavailable(String),
}

/// Map the legacy `buffer_ms` setting onto a PipeWire quantum.
///
/// The original's range is 10…100 ms (`sndDevices.h:200-201`, clamped at
/// `sndDevicesSet.cpp:541-545`); the result is a power of two in `[256, 2048]` because graph-wide
/// quantum negotiation behaves best with them. Note that PipeWire takes the *minimum* requested
/// latency across the graph, clamped to `default.clock.min-quantum … max-quantum`, so this can
/// only ever lower the graph quantum, never raise it.
#[must_use]
pub fn quantum_for_ms(ms: u32, rate: u32) -> u32 {
    let ms = ms.clamp(10, 100);
    let rate = rate.clamp(8_000, MAX_SAMPLE_RATE);
    let frames = u64::from(ms) * u64::from(rate) / 1000;
    let frames = u32::try_from(frames).unwrap_or(MAX_QUANTUM_FRAMES as u32);
    (frames.next_power_of_two() >> 1).clamp(256, MAX_QUANTUM_FRAMES as u32)
}

/// What the engine is started with ([`AudioEngine::start_with_options`]): what the settings file
/// says that has to be in place before the first pair of nodes is built, rather than arrive after
/// it on the control channel.
///
/// The output lane is enabled from the start and builds its pair as soon as the registry has been
/// read — a round trip after the connection, well before a message sent once
/// [`AudioEngine::start`] has returned is sure to have been taken in. Anything the first pair
/// depends on travels here: whether it is built at all, and whether it takes the session default,
/// included.
///
/// [`StartOptions::default`] is the engine as [`AudioEngine::start`] starts it: the output lane
/// on, both lanes wanting the default, nothing ranked or remembered.
#[derive(Debug, Clone, PartialEq)]
pub struct StartOptions {
    /// The UI's language for the virtual nodes' descriptions ([`AudioEngine::start_with_language`]);
    /// `None` reads the desktop locale.
    pub language: Option<String>,
    /// Every remembered per-target volume of CrossFX's own nodes: the settings file's
    /// `device_volumes` (U10). The first pair on a remembered device starts at its level; without
    /// them it would start at a level chosen without them — unity — and only move once [`UiToAudio::SeedTargetVolumes`] arrived. Sanitised
    /// here as that message's are.
    pub target_volumes: Vec<TargetVolume>,
    /// The output lane's device ranking (U4), as [`UiToAudio::SetDevicePriority`] would send it:
    /// the settings' ranked list of outputs, or an empty one while the app follows the system's
    /// default. Here so that the lane's first choice of device is already made by rank: made
    /// without it, by the original's rules, it would be the session default, and a ranking arriving
    /// a moment later would have to take the lane from there.
    pub output_priority: DevicePriority,
    /// The input lane's device ranking, as [`Self::output_priority`] is the output lane's.
    pub input_priority: DevicePriority,
    /// Whether both lanes take the session default once their nodes are up, as
    /// [`UiToAudio::SetAsDefault`] would say for each: `false` for an app whose power was left
    /// off, which keeps CrossFX out of the path (U12). Here so that the first pair does not claim
    /// the default, move every playing stream onto CrossFX and have them moved back a moment
    /// later, when a `SetAsDefault` with `want: false` arrives after it.
    ///
    /// [`UiToAudio::SetAsDefault`]: crossfx_core::messages::UiToAudio::SetAsDefault
    pub want_default: bool,
    /// Whether the output lane starts enabled: `false` for an app whose speakers' lane was left
    /// `Off`, as [`UiToAudio::DetachLane`] would say. Here so that no first pair is built on the
    /// speakers — the default claimed and the streams moved — before that message arrives. The
    /// input lane always starts detached, until a microphone is picked.
    ///
    /// [`UiToAudio::DetachLane`]: crossfx_core::messages::UiToAudio::DetachLane
    pub output_enabled: bool,
}

impl Default for StartOptions {
    fn default() -> Self {
        Self {
            language: None,
            target_volumes: Vec::new(),
            output_priority: DevicePriority::default(),
            input_priority: DevicePriority::default(),
            want_default: true,
            output_enabled: true,
        }
    }
}

/// One lane's ranking of its devices, as [`UiToAudio::SetDevicePriority`] carries it after start:
/// what [`StartOptions`] hands the engine for each lane's first choice.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DevicePriority {
    /// `node.name`s, most preferred first. Empty is "follow the system": the original's rules and
    /// the session default decide.
    pub names: Vec<String>,
    /// Whether a device the ranking does not name yet goes before every ranked one, or after
    /// every one: the app's "prioritize new output devices" ([`Preference::new_devices_first`]).
    pub new_devices_first: bool,
}

/// The control channel to the audio thread: PipeWire's own on Linux, whose sender wakes the
/// thread's loop, and on Windows `wasapi::Control`, a channel the WASAPI control thread waits on
/// beside its own events. Both have `send(message) -> Result<(), UiToAudio>`.
#[cfg(target_os = "linux")]
type ControlSender = pipewire::channel::Sender<UiToAudio>;
#[cfg(windows)]
type ControlSender = wasapi::Control;

/// The audio backend. Owns the audio thread and everything on it: the PipeWire thread on Linux,
/// the WASAPI thread on Windows.
///
/// There is no public constructor other than [`AudioEngine::start`], which hands back an
/// [`EngineHandle`] that owns this. The GUI never touches an `AudioEngine` directly, because
/// every PipeWire object (and every WASAPI client) it transitively owns is thread-affine.
pub struct AudioEngine {
    control: ControlSender,
    join: Option<JoinHandle<()>>,
}

impl AudioEngine {
    /// Spawn the audio thread.
    ///
    /// Returns as soon as the first connection attempt has resolved, which for the two outcomes
    /// that matter is immediate: a missing server fails at `connect(2)`, and a present one is
    /// connected in well under a millisecond. Everything after that — registry enumeration,
    /// device selection, creating the two nodes, taking the session default — happens in the
    /// background and is reported through [`EngineHandle::try_recv`].
    ///
    /// The engine starts with the output lane enabled and the input lane detached. The output lane
    /// attaches to the device the port of `sndDevicesImplementDeviceRules` picks
    /// ([`devices::choose_device`]) and, as soon as its nodes are up, makes `crossfx_sink` the
    /// configured default sink — remembering the previous default so it can be handed back
    /// (`docs/spec/12-audio-io.md` §21). Send [`UiToAudio::SelectDevice`] to attach a lane to a
    /// specific device — a microphone enables the input lane beside the output lane, never instead
    /// of it — [`UiToAudio::DetachLane`] to switch a lane off, and [`UiToAudio::SetAsDefault`]
    /// with `want: false` to keep a lane's default where it is.
    ///
    /// # Errors
    /// [`AudioError::PipewireUnavailable`] if there is no PipeWire session to connect to, or if
    /// the thread could not be spawned. Later failures are not errors here: the engine keeps
    /// retrying with the backoff in `docs/spec/12-audio-io.md` §22 and reports state through
    /// [`AudioToUi`].
    ///
    /// [`UiToAudio::SelectDevice`]: crossfx_core::messages::UiToAudio::SelectDevice
    /// [`UiToAudio::DetachLane`]: crossfx_core::messages::UiToAudio::DetachLane
    pub fn start() -> Result<EngineHandle, AudioError> {
        Self::start_with_options(StartOptions::default())
    }

    /// [`AudioEngine::start`], naming the language the virtual nodes are described in.
    ///
    /// `language` is one of `crossfx_core::i18n::LANGUAGES`' codes — the UI's effective language,
    /// so `CrossFX (Вывод)` in the sound settings matches a Russian CrossFX window even when the
    /// desktop locale says otherwise. `None` (plain [`AudioEngine::start`]) reads the locale.
    ///
    /// A caller with remembered per-device volumes starts with [`Self::start_with_options`]
    /// instead, so that the first pair already has them.
    pub fn start_with_language(language: &str) -> Result<EngineHandle, AudioError> {
        Self::start_with_options(StartOptions {
            language: Some(language.to_owned()),
            ..StartOptions::default()
        })
    }

    /// [`AudioEngine::start`], with everything the first pair of nodes depends on
    /// ([`StartOptions`]).
    #[cfg(target_os = "linux")]
    pub fn start_with_options(options: StartOptions) -> Result<EngineHandle, AudioError> {
        Self::start_with(
            None,
            options,
            aec::WEBRTC_LIBRARY,
            app_routes::ROUTE_IDLE,
            stream_handover::journal_file(),
        )
    }

    /// [`AudioEngine::start`], with everything the first pair of streams depends on
    /// ([`StartOptions`]): on Windows, the WASAPI backend (`wasapi`). What the options mean for
    /// PipeWire's nodes they mean for the loopback capture and the render stream there.
    ///
    /// # Errors
    /// [`AudioError::WindowsAudioUnavailable`] when the audio thread cannot be started.
    #[cfg(windows)]
    pub fn start_with_options(options: StartOptions) -> Result<EngineHandle, AudioError> {
        wasapi::start(options)
    }

    /// [`AudioEngine::start`], against a named PipeWire socket rather than the session default.
    ///
    /// Crate-internal because the public surface is fixed, but it is the seam the tests use: a
    /// remote name that does not exist exercises the whole spawn/connect/report path — including
    /// the error mapping and the thread shutdown — without a server and without creating a single
    /// node in the user's live graph.
    #[cfg(target_os = "linux")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn start_with_remote(remote: Option<&str>) -> Result<EngineHandle, AudioError> {
        Self::start_with(
            remote,
            StartOptions::default(),
            aec::WEBRTC_LIBRARY,
            app_routes::ROUTE_IDLE,
            None,
        )
    }

    /// [`Self::start_with_remote`], with a per-application route nobody uses kept for `idle`
    /// rather than [`app_routes::ROUTE_IDLE`]: a test watches a route go without waiting ten
    /// seconds for it.
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn start_with_route_idle(
        remote: Option<&str>,
        idle: Duration,
    ) -> Result<EngineHandle, AudioError> {
        Self::start_with(
            remote,
            StartOptions::default(),
            aec::WEBRTC_LIBRARY,
            idle,
            None,
        )
    }

    /// [`Self::start_with_remote`], with [`StartOptions`].
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn start_for_tests(
        remote: Option<&str>,
        options: StartOptions,
    ) -> Result<EngineHandle, AudioError> {
        Self::start_with(
            remote,
            options,
            aec::WEBRTC_LIBRARY,
            app_routes::ROUTE_IDLE,
            None,
        )
    }

    /// [`Self::start_with_remote`], with the handover's journal kept at `journal` — a file of the
    /// test's — rather than in memory: what a test of a run killed in the middle of a handover
    /// reads, and hands the next run.
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn start_with_journal(
        remote: Option<&str>,
        journal: std::path::PathBuf,
    ) -> Result<EngineHandle, AudioError> {
        Self::start_with(
            remote,
            StartOptions::default(),
            aec::WEBRTC_LIBRARY,
            app_routes::ROUTE_IDLE,
            Some(journal),
        )
    }

    /// [`Self::start_with_remote`], with the echo canceller running `library` rather than WebRTC.
    ///
    /// For the tests: `aec/libspa-aec-null` passes the microphone through untouched, which is all a
    /// test of where the canceller's nodes go needs, and a library that does not exist is how a
    /// test makes the load fail the way a missing `libspa-aec-webrtc` would.
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn start_with_canceller(
        remote: Option<&str>,
        library: &'static str,
    ) -> Result<EngineHandle, AudioError> {
        Self::start_with(
            remote,
            StartOptions::default(),
            library,
            app_routes::ROUTE_IDLE,
            None,
        )
    }

    #[cfg(target_os = "linux")]
    fn start_with(
        remote: Option<&str>,
        options: StartOptions,
        aec_library: &'static str,
        route_idle: Duration,
        handover_journal: Option<std::path::PathBuf>,
    ) -> Result<EngineHandle, AudioError> {
        // Whether the *socket* exists is left to `pw_context_connect`, which resolves
        // `remote.name` itself and reports the failure precisely. What has to be checked first is
        // the thing PipeWire cannot report usefully: no runtime directory at all, which is what a
        // bare TTY or a Flatpak without the `pipewire` socket permission looks like
        // (`docs/spec/12-audio-io.md` §22).
        engine::preflight(std::env::var_os("XDG_RUNTIME_DIR").as_deref())?;

        let StartOptions {
            language,
            target_volumes,
            output_priority,
            input_priority,
            want_default,
            output_enabled,
        } = options;
        let (mut handle, mut config, ready) = EngineHandle::wire(remote, language.as_deref());
        config.aec_library = aec_library;
        config.target_volumes = target_volumes;
        config.device_priority = PerDirection {
            output: output_priority,
            input: input_priority,
        };
        config.want_default = want_default;
        config.output_enabled = output_enabled;
        config.route_idle = route_idle;
        config.handover_journal = handover_journal;
        let join = std::thread::Builder::new()
            .name("crossfx-audio".to_owned())
            .spawn(move || engine::run(config))
            .map_err(|e| AudioError::PipewireUnavailable(e.to_string()))?;
        handle.engine.join = Some(join);

        match ready.recv_timeout(START_TIMEOUT) {
            // Connected, or still trying — either way the caller gets a working handle.
            Ok(Ok(())) | Err(RecvTimeoutError::Timeout) => Ok(handle),
            Ok(Err(error)) => {
                handle.shutdown();
                Err(error)
            }
            Err(RecvTimeoutError::Disconnected) => {
                handle.shutdown();
                Err(AudioError::PipewireUnavailable(
                    "the audio thread exited before it connected".to_owned(),
                ))
            }
        }
    }

    /// Ask the thread to stop and wait for it. Idempotent.
    fn stop(&mut self) {
        let Some(join) = self.join.take() else {
            return;
        };
        // If the send fails the loop is already gone; joining is still the right move.
        let _ = self.control.send(UiToAudio::Shutdown);
        if join.join().is_err() {
            log::error!("the CrossFX audio thread panicked");
        }
    }
}

impl std::fmt::Debug for AudioEngine {
    /// Hand-written because `pipewire::channel::Sender` is not `Debug`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioEngine")
            .field("running", &self.join.is_some())
            .finish()
    }
}

impl Drop for AudioEngine {
    /// Dropping the handle must never leave the virtual sink behind, so the thread is always
    /// joined — the PipeWire connection, and with it both nodes, is torn down before this returns.
    fn drop(&mut self) {
        self.stop();
    }
}

/// The GUI's view of the running engine: cheap, `Send`, safe to poll every frame.
///
/// Deliberately not `Clone`: the parameter and meter paths are single-producer and
/// single-consumer by construction, which is what makes them wait-free.
pub struct EngineHandle {
    engine: AudioEngine,
    params: Input<DspParams>,
    input_params: Input<InputDspParams>,
    /// Each lane's meters, through a buffer of its own.
    meters: PerDirection<Output<Meters>>,
    /// Each lane's event queue.
    events: PerDirection<Sender<DspEvent>>,
    notifications: Receiver<AudioToUi>,
}

impl std::fmt::Debug for EngineHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineHandle")
            .field("engine", &self.engine)
            .field("pending_notifications", &self.notifications.len())
            .finish_non_exhaustive()
    }
}

impl EngineHandle {
    /// Every path between the GUI and the audio thread, both ends, with no thread yet: the handle
    /// (its engine not yet joined to anything), the thread's [`engine::Config`], and the receiver
    /// the thread reports its first connection attempt on.
    ///
    /// The one place the two sides are paired, so a lane's meters buffer and event queue are
    /// created together with the lane's ends of them and cannot be crossed over. Also the seam the
    /// tests use to drive both lanes' DSP through a real handle without a server.
    #[cfg(target_os = "linux")]
    fn wire(
        remote: Option<&str>,
        language: Option<&str>,
    ) -> (Self, engine::Config, Receiver<Result<(), AudioError>>) {
        let (params_in, params_out) = TripleBuffer::new(&DspParams::default()).split();
        let (input_params_in, input_params_out) =
            TripleBuffer::new(&InputDspParams::default()).split();
        let (meters_in, meters_out) =
            PerDirection::from_fn(|_| TripleBuffer::new(&Meters::default()).split()).unzip();
        let (events_tx, events_rx) =
            PerDirection::from_fn(|_| crossbeam_channel::bounded(EVENT_QUEUE_LEN)).unzip();
        let (ui_tx, ui_rx) = crossbeam_channel::unbounded();
        let (control_tx, control_rx) = pipewire::channel::channel();
        let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);

        let config = engine::Config {
            remote: remote.map(str::to_owned),
            language: language.map(str::to_owned),
            control: control_rx,
            notify: ui_tx,
            params: params_out,
            input_params: input_params_out,
            meters: meters_in,
            events: events_rx,
            lane_events: PerDirection {
                output: events_tx.output.clone(),
                input: events_tx.input.clone(),
            },
            ready: ready_tx,
            aec_library: aec::WEBRTC_LIBRARY,
            target_volumes: Vec::new(),
            device_priority: PerDirection::default(),
            want_default: true,
            output_enabled: true,
            route_idle: app_routes::ROUTE_IDLE,
            handover_journal: None,
        };
        let handle = Self {
            engine: AudioEngine {
                control: control_tx,
                join: None,
            },
            params: params_in,
            input_params: input_params_in,
            meters: meters_out,
            events: events_tx,
            notifications: ui_rx,
        };
        (handle, config, ready_rx)
    }

    /// Publish a new parameter snapshot.
    ///
    /// Wait-free: [`triple_buffer::Input::write`] is a move into a spare buffer plus one atomic
    /// swap, so this is safe to call on every GUI frame and cannot make the audio thread wait. If
    /// the audio thread has not read the previous snapshot yet, that snapshot is simply
    /// superseded — parameters are state, not events.
    pub fn set_params(&mut self, mut params: DspParams) {
        // The single gate between everything that can produce a snapshot — sliders, the command
        // line, a preset file, `settings.toml` — and the filter designs. Doing it here rather
        // than in each producer means a new caller cannot forget it, and it costs one pass over a
        // `Copy` struct on the GUI thread, never on the audio thread.
        params.sanitise();
        self.params.write(params);
    }

    /// Publish a new snapshot for the microphone chain.
    ///
    /// The sibling of [`Self::set_params`], with the same wait-free guarantee and the same single
    /// gate: [`InputDspParams::sanitise`] runs here so that no producer of a voice preset can
    /// forget it. Separate from the output snapshot because the two chains share nothing but the
    /// equalizer — publishing both through one struct would mean every music preset carried a gate
    /// threshold, and the audio thread would have to know which half of its parameters to ignore.
    ///
    /// Publishing while the input lane has no nodes is harmless and deliberate: the snapshot is
    /// state, so the voice chain reads a current one the moment the lane is attached, and
    /// attaching needs no handshake.
    pub fn set_input_params(&mut self, mut params: InputDspParams) {
        params.sanitise();
        self.input_params.write(params);
    }

    /// Fire a one-shot event at one lane's chain: filter reset, spectrum reset, processed-time
    /// reset, capture-statistics reset.
    ///
    /// The lane is named here rather than in the event because each lane has its own queue, read
    /// by its own chain and by nothing else: a reset meant for the microphone cannot clear the
    /// music chain's history, whichever lanes are running. An event for a lane with no nodes is
    /// applied on the audio thread's main loop, so it is neither lost nor left waiting.
    ///
    /// Unlike a parameter snapshot these must not be coalesced, so they go through a bounded
    /// queue. If the queue is full — which needs 64 unconsumed events, i.e. an audio thread that
    /// is not running — the event is dropped and logged rather than blocking the GUI.
    pub fn send_event(&self, direction: DeviceDirection, event: DspEvent) {
        match self.events.get(direction).try_send(event) {
            Ok(()) => {}
            Err(TrySendError::Full(event)) => {
                log::warn!(
                    "dropping {event:?} for the {} lane: the audio thread is not draining its \
                     event queue",
                    direction.key()
                );
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    /// The most recent meters the lane's chain published.
    ///
    /// Each lane publishes through a buffer of its own, so the music chain's spectrum and the
    /// voice chain's gate reduction never overwrite each other. A lane that is not running keeps
    /// the last meters it published — [`Meters::default`] until it has run at all.
    ///
    /// Never blocks and never waits: one atomic swap. Returns the last published value again when
    /// nothing new has arrived, so a GUI that polls faster than the audio callback just redraws
    /// the same frame.
    #[must_use]
    pub fn meters(&mut self, direction: DeviceDirection) -> Meters {
        *self.meters.get_mut(direction).read()
    }

    /// Send a control-plane request. Non-blocking; wakes the PipeWire loop.
    pub fn send(&self, message: UiToAudio) {
        if self.engine.control.send(message).is_err() {
            log::warn!("the CrossFX audio thread is gone; control message dropped");
        }
    }

    /// Take the next control-plane notification, if any. Never blocks.
    #[must_use]
    pub fn try_recv(&self) -> Option<AudioToUi> {
        self.notifications.try_recv().ok()
    }

    /// The notification channel itself, for a caller that waits on it rather than polling
    /// [`Self::try_recv`]: the application's thread that wakes the GUI when the audio thread has
    /// something to say (0.4.0 design §12).
    ///
    /// A second end of the same channel, not a copy of what is on it: each notification goes to
    /// whichever end takes it first, so a caller that takes this one reads only from it.
    #[must_use]
    pub fn notifications(&self) -> Receiver<AudioToUi> {
        self.notifications.clone()
    }

    /// Stop the engine and wait for the PipeWire thread to finish tearing down.
    ///
    /// Tearing down in order matters: every session default CrossFX holds — the sink, the source,
    /// or both — is handed back to a real device *before* the nodes are destroyed, so there is
    /// never a window in which a default points at a node that no longer exists
    /// (`docs/spec/12-audio-io.md` §21.5). The hand-back is confirmed by a server round trip
    /// before the socket closes — libpipewire does not flush on disconnect — so this returns only
    /// once `default.configured.audio.*` really has been rewritten, or after a short bounded wait
    /// when the server is not there to answer. It is therefore also what a `SIGTERM`/`SIGINT`
    /// handler in the application must reach before the process exits.
    pub fn shutdown(mut self) {
        self.engine.stop();
    }
}

// Both against PipeWire: a private daemon for the graph tests, the PipeWire seams for the rest.
#[cfg(all(test, target_os = "linux"))]
mod graph_churn;

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    const fn assert_send<T: Send>() {}

    #[test]
    fn the_handle_can_be_moved_to_the_gui_thread() {
        assert_send::<EngineHandle>();
        assert_send::<AudioEngine>();
    }

    /// The task's central real-time requirement, asserted against the types rather than against
    /// behaviour: if the parameter path is a `triple_buffer::Input`/`Output` pair of a `Copy`
    /// payload, then by construction the audio thread's read is one atomic swap — no mutex, no
    /// allocation, no `Drop` to run.
    #[test]
    fn the_parameter_snapshot_path_is_wait_free_and_lock_free_by_construction() {
        fn param_channel_is_a_triple_buffer(handle: &mut EngineHandle) {
            let _: &mut Input<DspParams> = &mut handle.params;
            let _: &mut Input<InputDspParams> = &mut handle.input_params;
            // Both lanes, not just the one a single-lane engine used to have: each lane's meters
            // come back through a triple buffer of its own, and each lane's events go out through
            // a sender of its own.
            for direction in DeviceDirection::ALL {
                let _: &mut Output<Meters> = handle.meters.get_mut(direction);
                let _: &Sender<DspEvent> = handle.events.get(direction);
            }
        }
        let _ = param_channel_is_a_triple_buffer;

        // A bounded channel of non-zero capacity is crossbeam's array flavour: pre-allocated,
        // and `try_recv` on the audio thread never allocates and never parks. Checked per lane.
        let (handle, _config, _ready) = EngineHandle::wire(None, None);
        for (direction, events) in handle.events.iter() {
            assert_eq!(
                events.capacity(),
                Some(EVENT_QUEUE_LEN),
                "the {} lane's event queue must be a bounded array channel",
                direction.key()
            );
        }

        // A payload that owns heap memory would make the audio thread's `write`/`read` drop an
        // allocation. `Copy` rules that out for good. The microphone chain's snapshot is held to
        // the same standard as the music chain's — it reaches the same thread by the same path.
        const fn assert_copy<T: Copy>() {}
        assert_copy::<DspParams>();
        assert_copy::<InputDspParams>();
        assert_copy::<Meters>();

        // And the mechanism itself: the newest snapshot always arrives, and reading never blocks
        // even while the writer is hammering.
        let (mut input, mut output) = TripleBuffer::new(&DspParams::default()).split();
        let writer = std::thread::spawn(move || {
            for i in 0..10_000_u32 {
                input.write(DspParams {
                    master_gain_db: i as f32,
                    ..DspParams::default()
                });
            }
            input
        });
        let mut seen = 0_u32;
        for _ in 0..10_000 {
            seen = seen.max(output.read().master_gain_db as u32);
        }
        let mut input = writer.join().expect("writer thread");
        input.write(DspParams {
            master_gain_db: -1.0,
            ..DspParams::default()
        });
        assert_eq!(
            output.read().master_gain_db,
            -1.0,
            "the reader must always end up with the most recently written snapshot"
        );
        assert!(seen <= 9_999);
    }

    /// A handle wired to both lanes' DSP, as `engine::run` builds it, with no thread and no
    /// server: the GUI's ends and the audio thread's ends of the same paths.
    fn wired_lanes() -> (EngineHandle, PerDirection<lane_dsp::LaneDsp>) {
        let (handle, config, _ready) = EngineHandle::wire(None, None);
        let (lanes, _handover) = lane_dsp::build(
            config.params,
            config.input_params,
            config.meters,
            config.events,
        );
        (handle, lanes)
    }

    /// Run a block of a steady tone through a lane, so its counters have something to reset.
    fn run_a_block(dsp: &mut lane_dsp::LaneDsp) {
        let block: Vec<u8> = (0..512 * 2)
            .flat_map(|i| (0.25 * ((i / 2) as f32 * 0.05).sin()).to_le_bytes())
            .collect();
        dsp.refresh();
        dsp.process_bytes(&block, 2)
            .expect("a quantum of stereo fits the scratch");
    }

    #[test]
    fn an_event_sent_to_one_lane_is_never_consumed_by_the_other_lanes_engine() {
        let (handle, mut lanes) = wired_lanes();
        for (_, dsp) in lanes.iter_mut() {
            dsp.set_format(48_000.0, 2);
            run_a_block(dsp);
            assert!(dsp.meters().processed_samples > 0);
        }

        // A reset for the microphone. The music chain runs first and must leave it where it is.
        handle.send_event(DeviceDirection::Input, DspEvent::ResetProcessedTime);
        lanes.output.refresh();
        assert!(
            lanes.output.meters().processed_samples > 0,
            "the music chain consumed an event addressed to the voice chain"
        );
        lanes.input.refresh();
        assert_eq!(
            lanes.input.meters().processed_samples,
            0,
            "the voice chain never received its own event"
        );

        // And the other way round.
        run_a_block(&mut lanes.input);
        handle.send_event(DeviceDirection::Output, DspEvent::ResetProcessedTime);
        lanes.input.refresh();
        assert!(
            lanes.input.meters().processed_samples > 0,
            "the voice chain consumed an event addressed to the music chain"
        );
        lanes.output.refresh();
        assert_eq!(lanes.output.meters().processed_samples, 0);
    }

    #[test]
    fn an_event_that_fills_one_lanes_queue_leaves_the_other_lanes_queue_empty() {
        let (handle, config, _ready) = EngineHandle::wire(None, None);
        for _ in 0..EVENT_QUEUE_LEN {
            handle.send_event(DeviceDirection::Output, DspEvent::ResetFilterState);
        }
        assert!(config.events.output.is_full());
        assert!(
            config.events.input.is_empty(),
            "the voice chain's queue has room for its own events whatever the music chain's holds"
        );
        // Full is dropped and logged, never blocking the GUI, and still never spills over.
        handle.send_event(DeviceDirection::Output, DspEvent::ResetFilterState);
        assert!(config.events.input.is_empty());
        handle.send_event(DeviceDirection::Input, DspEvent::ResetCaptureStats);
        assert_eq!(config.events.input.len(), 1);
    }

    #[test]
    fn each_lanes_meters_are_published_through_its_own_buffer_and_do_not_overwrite_the_others() {
        let (mut handle, mut lanes) = wired_lanes();
        assert_eq!(handle.meters(DeviceDirection::Output), Meters::default());
        assert_eq!(handle.meters(DeviceDirection::Input), Meters::default());

        // Two rates no default could be mistaken for, one per lane.
        lanes.output.set_format(44_100.0, 2);
        lanes.input.set_format(96_000.0, 2);
        lanes.output.publish_meters();
        assert_eq!(handle.meters(DeviceDirection::Output).sample_rate, 44_100);
        assert_eq!(
            handle.meters(DeviceDirection::Input),
            Meters::default(),
            "the music chain's meters reached the voice chain's buffer"
        );

        lanes.input.publish_meters();
        assert_eq!(handle.meters(DeviceDirection::Input).sample_rate, 96_000);
        assert_eq!(
            handle.meters(DeviceDirection::Output).sample_rate,
            44_100,
            "the voice chain's meters overwrote the music chain's"
        );

        // A lane that publishes again moves only its own reading.
        lanes.output.set_format(48_000.0, 2);
        lanes.output.publish_meters();
        assert_eq!(handle.meters(DeviceDirection::Output).sample_rate, 48_000);
        assert_eq!(handle.meters(DeviceDirection::Input).sample_rate, 96_000);
    }

    #[test]
    fn the_buffer_length_setting_maps_onto_power_of_two_quanta() {
        // The original's range, `sndDevices.h:200-201`.
        assert_eq!(quantum_for_ms(10, 48_000), 256);
        assert_eq!(quantum_for_ms(100, 48_000), 2048);
        // Out-of-range values clamp exactly as `sndDevicesSetBufferSizeMilliSecs` does.
        assert_eq!(quantum_for_ms(0, 48_000), quantum_for_ms(10, 48_000));
        assert_eq!(quantum_for_ms(10_000, 48_000), quantum_for_ms(100, 48_000));
        // The original's compiled-in default of 80 ms lands on the spec's `Safe` row.
        assert_eq!(quantum_for_ms(80, 48_000), 2048);
        // Every result is a power of two inside the range the ring is sized for.
        for ms in 1..=200 {
            for rate in [44_100, 48_000, 96_000, 192_000] {
                let q = quantum_for_ms(ms, rate);
                assert!(q.is_power_of_two(), "{ms}ms @ {rate}Hz gave {q}");
                assert!((256..=MAX_QUANTUM_FRAMES as u32).contains(&q));
            }
        }
    }

    #[test]
    fn every_node_crossfx_makes_is_its_own_and_nobody_elses_is() {
        for name in OUR_NODE_NAMES {
            assert!(is_crossfx_node(name), "{name}");
            assert!(name.starts_with(NODE_PREFIX), "{name}");
        }
        // Every node of ours starts with the prefix the WirePlumber hook tells ours by.
        assert_eq!(NODE_PREFIX, "crossfx_");
        for prefix in [ROUTE_NODE_PREFIX, AEC_NODE_PREFIX] {
            assert!(prefix.starts_with(NODE_PREFIX), "{prefix}");
        }
        // A per-application route's four kinds of node, whatever its number.
        for name in [
            "crossfx_route_o1",
            "crossfx_route_o1_play",
            "crossfx_route_i4_capture",
            "crossfx_route_i4",
            "crossfx_route_o17",
        ] {
            assert!(is_crossfx_node(name), "{name}");
        }
        // The canceller's, by its prefix, including one it does not make today.
        assert!(is_crossfx_node("crossfx_aec_reference"));
        assert_eq!(AEC_NODE_PREFIX, "crossfx_aec_");
        assert_eq!(ROUTE_NODE_PREFIX, "crossfx_route_");
        for name in AEC_NODE_NAMES {
            assert!(name.starts_with(AEC_NODE_PREFIX), "{name}");
        }
        // Somebody else's, even with our word in it.
        for name in [
            "",
            "crossfx",
            "crossfx_sink_2",
            "crossfx_router",
            "my_crossfx_route_o1",
            "alsa_output.pci-0000_00_1f.3.analog-stereo",
            "Firefox",
        ] {
            assert!(!is_crossfx_node(name), "{name:?}");
        }
    }

    #[test]
    fn the_node_names_the_default_metadata_will_carry_are_stable() {
        assert_eq!(SINK_NODE_NAME, "crossfx_sink");
        assert_eq!(OUTPUT_NODE_NAME, "crossfx_output");
        assert_eq!(CAPTURE_NODE_NAME, "crossfx_capture");
        assert_eq!(SOURCE_NODE_NAME, "crossfx_source");
        assert_eq!(LINK_GROUP, "crossfx");
        assert_eq!(link_group(DeviceDirection::Output), LINK_GROUP);
        assert_eq!(link_group(DeviceDirection::Input), INPUT_LINK_GROUP);
        assert_ne!(
            LINK_GROUP, INPUT_LINK_GROUP,
            "one group for both lanes keeps the speakers running for as long as the microphone is"
        );
        assert_eq!(our_node_name(DeviceDirection::Output), SINK_NODE_NAME);
        assert_eq!(our_node_name(DeviceDirection::Input), SOURCE_NODE_NAME);
        // The echo canceller's three, which are ours as much as the lanes' four: never a device.
        assert_eq!(AEC_CAPTURE_NODE_NAME, "crossfx_aec_capture");
        assert_eq!(AEC_MONITOR_NODE_NAME, "crossfx_aec_monitor");
        assert_eq!(AEC_SOURCE_NODE_NAME, "crossfx_aec_source");
        assert_eq!(AEC_LINK_GROUP, "crossfx-aec");
        assert_eq!(
            OUR_NODE_NAMES.to_vec(),
            [
                LANE_NODE_NAMES.as_slice(),
                AEC_NODE_NAMES.as_slice(),
                &[KEEP_AWAKE_NODE_NAME]
            ]
            .concat(),
            "every node of ours is a lane's, the canceller's, or the one that keeps the microphone \
             awake"
        );
        // Names are matched by string and written into metadata: ASCII, no spaces, all distinct.
        for name in OUR_NODE_NAMES {
            assert!(name.is_ascii() && !name.contains(' '), "{name}");
        }
        let mut sorted = OUR_NODE_NAMES.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), OUR_NODE_NAMES.len());
        // The descriptions, by contrast, are for people and carry the localised direction.
        assert_eq!(SINK_DESCRIPTION, "CrossFX");
        assert_eq!(SINK_DESCRIPTION, crossfx_core::identity::PRODUCT);
        assert_eq!(OUTPUT_STREAM_DESCRIPTION, "CrossFX output");
        assert_eq!(CAPTURE_STREAM_DESCRIPTION, "CrossFX capture");
        assert_eq!(KEEP_AWAKE_NODE_NAME, "crossfx_mic_check");
        assert_eq!(KEEP_AWAKE_STREAM_DESCRIPTION, "CrossFX microphone check");
        for description in [
            OUTPUT_STREAM_DESCRIPTION,
            CAPTURE_STREAM_DESCRIPTION,
            KEEP_AWAKE_STREAM_DESCRIPTION,
        ] {
            assert!(description.starts_with(SINK_DESCRIPTION), "{description}");
        }
        assert_eq!(
            node_description(DeviceDirection::Output, Some("ru")),
            "CrossFX (Вывод)"
        );
        assert_eq!(
            node_description(DeviceDirection::Input, Some("ru")),
            "CrossFX (Ввод)"
        );
    }

    /// Constructing against a socket that does not exist must return an error rather than panic,
    /// and must not leave a thread or a node behind. Nothing in this test touches the live graph.
    #[test]
    fn starting_without_a_pipewire_server_returns_an_error_instead_of_panicking() {
        let error = AudioEngine::start_with_remote(Some("crossfx-no-such-socket-for-tests"))
            .expect_err("connecting to a socket that does not exist must fail");
        let AudioError::PipewireUnavailable(detail) = &error else {
            panic!("expected PipewireUnavailable, got {error:?}");
        };
        assert!(
            !detail.is_empty(),
            "the message shown to the user must say something"
        );
        assert!(error.to_string().starts_with("PipeWire is not available"));
    }

    #[test]
    fn a_missing_runtime_directory_is_reported_rather_than_retried_forever() {
        use std::ffi::OsStr;

        assert!(matches!(
            engine::preflight(None),
            Err(AudioError::PipewireUnavailable(_))
        ));
        assert!(matches!(
            engine::preflight(Some(OsStr::new(""))),
            Err(AudioError::PipewireUnavailable(_))
        ));
        assert!(matches!(
            engine::preflight(Some(OsStr::new("/definitely/not/a/runtime/dir"))),
            Err(AudioError::PipewireUnavailable(_))
        ));
        // A directory that does exist passes; whether a server is listening in it is
        // `pw_context_connect`'s business.
        assert!(engine::preflight(Some(OsStr::new("/tmp"))).is_ok());
    }
}
