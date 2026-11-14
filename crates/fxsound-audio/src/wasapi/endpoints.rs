//! The endpoints the MMDevice API reports, and what each one is to FxSound: a real device the user
//! can pick, or a virtual one a lane stands on.
//!
//! Pure: [`Endpoint`] is plain data read out of an endpoint's property store
//! (`super::com::Enumerator::endpoints`), and everything here is a function of a list of them, so
//! the rules are tested against endpoints written out by hand rather than against the machine the
//! tests run on.
//!
//! # The virtual devices
//!
//! FxSound for Windows ships a kernel driver, "FxSound Audio Enhancer", whose one playback
//! endpoint, "FxSound Speakers", is what it makes every application play into
//! (`docs/spec/12-audio-io.md` §1, §6). This port ships no driver. It stands on one that is
//! installed already: FxSound's own, when the original is (or was) installed, and otherwise a
//! virtual cable — VB-Audio's, or Eugene Muzychenko's Virtual Audio Cable. Each is found the way the
//! original finds its own, by name and never by a GUID: the original matches the substring
//! `"FxSound Audio Enhancer"` in the friendly name (`sndDevices_GetAll.cpp:238-247`), because
//! Windows may put `"2- "` in front of a second device of the same name, and so does
//! [`Endpoint::virtual_kind`]. A cable is recognised by its adapter's name
//! (`PKEY_DeviceInterface_FriendlyName`), which the cable's driver sets and Windows never
//! translates, unlike the endpoints' own names ("Динамики", "Lautsprecher").
//!
//! The output lane needs one playback endpoint: applications play into it, and the lane captures
//! it with WASAPI loopback. The input lane needs a cable: it plays the processed microphone into
//! the cable's playback side, and applications record it from the cable's recording side — its
//! twin ([`capture_twin`]). FxSound's driver has no such twin: its recording endpoints ("Microphone",
//! "Internal AUX", "MIDI") are the driver sample's own, not a loop of its speakers. So the input
//! lane needs a cable of its own, distinct from the output lane's ([`plan`]).
//!
//! Every endpoint of a virtual device is FxSound's in the sense `crate::is_fxsound_node` gives a
//! node of the Linux engine: never a device to pick. Offered as an output, the virtual device would
//! have the output lane play into what it captures; a cable's recording side offered as a
//! microphone, the input lane record what it plays ([`is_offered`]).

use fxsound_core::{AudioDevice, DeviceDirection};

use crate::devices::{BluezFacts, ChannelMap, DeviceInfo, FormFactor};

/// One active endpoint, as its property store describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Endpoint {
    /// The endpoint ID string (`IMMDevice::GetId`): the stable identity, what the settings keep and
    /// what [`AudioDevice::name`] carries — the analogue of a PipeWire `node.name`.
    pub(crate) id: String,
    /// Render endpoints are outputs, capture endpoints inputs.
    pub(crate) direction: DeviceDirection,
    /// `PKEY_Device_FriendlyName`: "Speakers (Realtek(R) Audio)", what the device list shows.
    pub(crate) friendly_name: String,
    /// `PKEY_Device_DeviceDesc`: "Speakers".
    pub(crate) description: String,
    /// `PKEY_DeviceInterface_FriendlyName`: the adapter, "Realtek(R) Audio".
    pub(crate) interface_name: String,
    /// `PKEY_Device_EnumeratorName`: the bus the device is on — `HDAUDIO`, `USB`, `BTHENUM`
    /// (Bluetooth A2DP), `BTHHFENUM` (Bluetooth hands-free), `ROOT` (a software device).
    pub(crate) enumerator: String,
    /// `PKEY_Device_ContainerId`: the physical device the endpoint belongs to, shared by every
    /// endpoint of one headset.
    pub(crate) container: Option<String>,
    /// `PKEY_AudioEndpoint_FormFactor`, an `EndpointFormFactor`.
    pub(crate) form_factor: Option<u32>,
}

/// What kind of virtual device an endpoint belongs to, best first: the order a lane prefers them in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum VirtualKind {
    /// FxSound's own driver, "FxSound Audio Enhancer", which the original installs — and "DFX
    /// Audio Enhancer", the name it had before (`sndDevices.h:52`). Playback only.
    FxSound,
    /// VB-Audio Virtual Cable: "CABLE Input" and "CABLE Output".
    VbCable,
    /// VB-Audio's other cables: A to D, and the Hi-Fi Cable.
    VbAudioCable,
    /// Virtual Audio Cable, Eugene Muzychenko's: "Line 1", "Line 2"…
    VirtualAudioCable,
}

impl VirtualKind {
    /// Whether the device has a recording side that records what its playback side plays: what
    /// the input lane needs. FxSound's driver does not.
    pub(crate) const fn is_cable(self) -> bool {
        match self {
            Self::FxSound => false,
            Self::VbCable | Self::VbAudioCable | Self::VirtualAudioCable => true,
        }
    }
}

/// The substrings that name FxSound's driver, in either of its names.
const FXSOUND_DRIVERS: [&str; 2] = ["fxsound audio enhancer", "dfx audio enhancer"];

impl Endpoint {
    /// Which virtual device this endpoint is part of, if any (module docs, "The virtual devices").
    pub(crate) fn virtual_kind(&self) -> Option<VirtualKind> {
        let friendly = self.friendly_name.to_lowercase();
        let adapter = self.interface_name.to_lowercase();
        if FXSOUND_DRIVERS
            .iter()
            .any(|driver| friendly.contains(driver) || adapter.contains(driver))
        {
            return Some(VirtualKind::FxSound);
        }
        let adapter = if adapter.is_empty() {
            // An endpoint whose adapter says nothing is named after it in brackets:
            // "CABLE Input (VB-Audio Virtual Cable)".
            friendly
                .rsplit_once('(')
                .map(|(_, inside)| inside.trim_end_matches(')').trim().to_owned())
                .unwrap_or_default()
        } else {
            adapter
        };
        let adapter = adapter.trim();
        if adapter.starts_with("vb-audio virtual cable") {
            Some(VirtualKind::VbCable)
        } else if adapter.starts_with("vb-audio cable ")
            || adapter.starts_with("vb-audio hi-fi cable")
        {
            Some(VirtualKind::VbAudioCable)
        } else if adapter == "virtual audio cable" || adapter.starts_with("virtual audio cable ") {
            Some(VirtualKind::VirtualAudioCable)
        } else {
            None
        }
    }

    /// Whether the endpoint is a Bluetooth device's: on the A2DP or the hands-free enumerator, or
    /// LE audio's.
    pub(crate) fn is_bluetooth(&self) -> bool {
        let enumerator = self.enumerator.to_ascii_uppercase();
        enumerator.starts_with("BTH") || enumerator == "BLUETOOTHLEDEVICE"
    }

    /// How the GUI should picture the endpoint: the eleven `EndpointFormFactor`s are exactly the
    /// eleven [`FormFactor`]s (`sndDevices_GetAll.cpp:198-208`), and an input that says it is
    /// speakers or nothing is a microphone, as on Linux.
    pub(crate) fn form_factor(&self) -> FormFactor {
        let form = match self.form_factor {
            Some(0) => FormFactor::NetworkDevice,
            Some(1) => FormFactor::Speakers,
            Some(2) => FormFactor::LineLevel,
            Some(3) => FormFactor::Headphones,
            Some(4) => FormFactor::Microphone,
            Some(5) => FormFactor::Headset,
            Some(6) => FormFactor::Handset,
            Some(7) => FormFactor::DigitalPassthrough,
            Some(8) => FormFactor::Spdif,
            Some(9) => FormFactor::Hdmi,
            _ => FormFactor::Unknown,
        };
        match (self.direction, form) {
            (DeviceDirection::Input, FormFactor::Speakers | FormFactor::Unknown) => {
                FormFactor::Microphone
            }
            (_, form) => form,
        }
    }

    /// What the GUI is told about the endpoint, `id` being the runtime handle the engine gave it.
    /// `default` is the current default endpoint of its direction.
    pub(crate) fn to_audio_device(&self, id: u32, default: Option<&str>) -> AudioDevice {
        AudioDevice {
            id,
            name: self.id.clone(),
            description: self.label().to_owned(),
            is_default: default == Some(self.id.as_str()),
            direction: self.direction,
            form_factor: self.form_factor().key().to_owned(),
        }
    }

    /// The name to show: the friendly name, or the description, or the ID when the store has
    /// neither — the original labelled such a device "Unknown" (`sndDevices_GetAll.cpp:260-262`),
    /// which says less than the ID does.
    pub(crate) fn label(&self) -> &str {
        [&self.friendly_name, &self.description, &self.id]
            .into_iter()
            .find(|name| !name.is_empty())
            .map_or("", |name| name.as_str())
    }

    /// The endpoint as the device rules see a device (`crate::devices`): named by its ID, heard
    /// whenever it is active — which is all an enumeration of active endpoints lists.
    pub(crate) fn to_device_info(&self, id: u32) -> DeviceInfo {
        DeviceInfo {
            object_id: id,
            object_serial: None,
            card_id: None,
            profile_device: None,
            available: true,
            bluez_address: None,
            name: self.id.clone(),
            description: self.label().to_owned(),
            nick: if self.description.is_empty() {
                self.label().to_owned()
            } else {
                self.description.clone()
            },
            channels: 0,
            rate: None,
            bluez_headset: false,
            bluez: BluezFacts::default(),
            positions: ChannelMap::default_for(2),
            form_factor: self.form_factor(),
            direction: self.direction,
        }
    }
}

/// Whether an endpoint is offered to the user as a device to pick: every one that is not part of
/// a virtual device (module docs).
pub(crate) fn is_offered(endpoint: &Endpoint) -> bool {
    endpoint.virtual_kind().is_none()
}

/// The recording side of the cable whose playback side is `render`: the recording endpoint of the
/// same kind and the same adapter — and, when the adapter has several (Virtual Audio Cable's
/// lines share one), of the same description too. `None` for FxSound's driver, whose recording
/// endpoints record nothing it plays, and for a cable whose recording side is disabled.
pub(crate) fn capture_twin<'a>(
    render: &Endpoint,
    endpoints: &'a [Endpoint],
) -> Option<&'a Endpoint> {
    let kind = render.virtual_kind().filter(|kind| kind.is_cable())?;
    let same_adapter: Vec<&Endpoint> = endpoints
        .iter()
        .filter(|endpoint| {
            endpoint.direction == DeviceDirection::Input
                && endpoint.virtual_kind() == Some(kind)
                && endpoint
                    .interface_name
                    .eq_ignore_ascii_case(&render.interface_name)
        })
        .collect();
    match same_adapter.as_slice() {
        [one] => Some(one),
        several => several.iter().copied().find(|endpoint| {
            endpoint
                .description
                .eq_ignore_ascii_case(&render.description)
        }),
    }
}

/// A virtual device a lane stands on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VirtualDevice {
    pub(crate) kind: VirtualKind,
    /// The playback endpoint: what applications play into, for the output lane; what the input
    /// lane plays into.
    pub(crate) render: String,
    /// Its name, for the log.
    pub(crate) name: String,
    /// The recording endpoint applications record the input lane from: a cable's twin. `None` for
    /// the output lane's device, whose recording side nothing uses.
    pub(crate) capture: Option<String>,
}

impl VirtualDevice {
    /// The endpoint whose default the lane of `direction` takes: the playback side for the output
    /// lane, the recording side for the input lane.
    pub(crate) fn default_endpoint(&self, direction: DeviceDirection) -> Option<&str> {
        match direction {
            DeviceDirection::Output => Some(&self.render),
            DeviceDirection::Input => self.capture.as_deref(),
        }
    }
}

/// Which virtual device each lane stands on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct VirtualPlan {
    pub(crate) output: Option<VirtualDevice>,
    pub(crate) input: Option<VirtualDevice>,
}

impl VirtualPlan {
    pub(crate) const fn get(&self, direction: DeviceDirection) -> Option<&VirtualDevice> {
        match direction {
            DeviceDirection::Output => self.output.as_ref(),
            DeviceDirection::Input => self.input.as_ref(),
        }
    }
}

/// Choose each lane's virtual device among `endpoints`.
///
/// The output lane takes the best playback endpoint of a virtual device ([`VirtualKind`]'s order,
/// then the name, so that the choice does not change with the order the endpoints are listed in).
/// The input lane takes the best cable with a recording side that is not the output lane's — while
/// the output lane is on: with the speakers' lane off, a machine with one cable can process its
/// microphone through it. The output lane's choice is made whether or not it is on, because it is
/// also the device a default left behind by a killed run is looked for on.
///
/// The input lane keeps the cable it stands on (`input_on`, its playback endpoint) for as long as
/// that is still one it may have: a cable that sorts first, or that the speakers' lane has just
/// freed, is no reason to move the recording default — and every application recording from it —
/// to another. The output lane keeps no such claim: FxSound's own driver, installed while the
/// lane runs on a cable, is the device the original would stand on, and the lane moves there.
pub(crate) fn plan(
    endpoints: &[Endpoint],
    output_enabled: bool,
    input_on: Option<&str>,
) -> VirtualPlan {
    let mut renders: Vec<(VirtualKind, &Endpoint)> = endpoints
        .iter()
        .filter(|endpoint| endpoint.direction == DeviceDirection::Output)
        .filter_map(|endpoint| endpoint.virtual_kind().map(|kind| (kind, endpoint)))
        .collect();
    renders.sort_by(|(a_kind, a), (b_kind, b)| {
        a_kind
            .cmp(b_kind)
            .then_with(|| a.label().cmp(b.label()))
            .then_with(|| a.id.cmp(&b.id))
    });
    let output = renders.first().map(|&(kind, endpoint)| VirtualDevice {
        kind,
        render: endpoint.id.clone(),
        name: endpoint.label().to_owned(),
        capture: None,
    });
    let taken = output
        .as_ref()
        .filter(|_| output_enabled)
        .map(|device| device.render.clone());
    let free = |&&(kind, endpoint): &&(VirtualKind, &Endpoint)| {
        kind.is_cable() && taken.as_deref() != Some(endpoint.id.as_str())
    };
    let kept = renders
        .iter()
        .filter(free)
        .find(|(_, endpoint)| input_on == Some(endpoint.id.as_str()));
    let input = kept
        .into_iter()
        .chain(renders.iter().filter(free))
        .find_map(|&(kind, endpoint)| {
            capture_twin(endpoint, endpoints).map(|twin| VirtualDevice {
                kind,
                render: endpoint.id.clone(),
                name: endpoint.label().to_owned(),
                capture: Some(twin.id.clone()),
            })
        });
    VirtualPlan { output, input }
}

/// Whether the output lane's device and the input lane's are one Bluetooth headset — its
/// headphones and its microphone — which Windows switches to the hands-free profile, mono and 8 or
/// 16 kHz, whenever the microphone records: the Windows face of what the Linux engine warns of
/// with [`crate::ONE_HEADSET_ON_BOTH_LANES`]. One headset is one container: every endpoint of a
/// physical device shares its `PKEY_Device_ContainerId`.
pub(crate) fn one_headset(output: &Endpoint, input: &Endpoint) -> bool {
    output.is_bluetooth()
        && input.is_bluetooth()
        && output.container.is_some()
        && output.container == input.container
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An endpoint written out the way this machine's property stores describe them.
    pub(crate) fn endpoint(
        id: &str,
        direction: DeviceDirection,
        friendly: &str,
        description: &str,
        adapter: &str,
    ) -> Endpoint {
        Endpoint {
            id: id.to_owned(),
            direction,
            friendly_name: friendly.to_owned(),
            description: description.to_owned(),
            interface_name: adapter.to_owned(),
            enumerator: "HDAUDIO".to_owned(),
            container: None,
            form_factor: Some(match direction {
                DeviceDirection::Output => 1,
                DeviceDirection::Input => 4,
            }),
        }
    }

    pub(crate) const OUT: DeviceDirection = DeviceDirection::Output;
    pub(crate) const IN: DeviceDirection = DeviceDirection::Input;

    pub(crate) fn speakers() -> Endpoint {
        endpoint(
            "{0.0.0.00000000}.{037f540c}",
            OUT,
            "Динамики (Realtek(R) Audio)",
            "Динамики",
            "Realtek(R) Audio",
        )
    }

    pub(crate) fn fxsound() -> Endpoint {
        Endpoint {
            enumerator: "ROOT".to_owned(),
            ..endpoint(
                "{0.0.0.00000000}.{73ca9fe5}",
                OUT,
                "FxSound Speakers (FxSound Audio Enhancer)",
                "FxSound Speakers",
                "FxSound Audio Enhancer",
            )
        }
    }

    pub(crate) fn microphone() -> Endpoint {
        endpoint(
            "{0.0.1.00000000}.{c280c4f6}",
            IN,
            "Набор микрофонов (Realtek(R) Audio)",
            "Набор микрофонов",
            "Realtek(R) Audio",
        )
    }

    pub(crate) fn cable() -> [Endpoint; 2] {
        [
            endpoint(
                "{0.0.0.00000000}.{cable-in}",
                OUT,
                "CABLE Input (VB-Audio Virtual Cable)",
                "CABLE Input",
                "VB-Audio Virtual Cable",
            ),
            endpoint(
                "{0.0.1.00000000}.{cable-out}",
                IN,
                "CABLE Output (VB-Audio Virtual Cable)",
                "CABLE Output",
                "VB-Audio Virtual Cable",
            ),
        ]
    }

    pub(crate) fn cable_a() -> [Endpoint; 2] {
        [
            endpoint(
                "{0.0.0.00000000}.{cable-a-in}",
                OUT,
                "CABLE-A Input (VB-Audio Cable A)",
                "CABLE-A Input",
                "VB-Audio Cable A",
            ),
            endpoint(
                "{0.0.1.00000000}.{cable-a-out}",
                IN,
                "CABLE-A Output (VB-Audio Cable A)",
                "CABLE-A Output",
                "VB-Audio Cable A",
            ),
        ]
    }

    #[test]
    fn fxsound_s_driver_is_found_by_its_name_whatever_windows_puts_in_front_of_it() {
        assert_eq!(fxsound().virtual_kind(), Some(VirtualKind::FxSound));
        let second = Endpoint {
            friendly_name: "FxSound Speakers (2- FxSound Audio Enhancer)".to_owned(),
            interface_name: String::new(),
            ..fxsound()
        };
        assert_eq!(second.virtual_kind(), Some(VirtualKind::FxSound));
        let legacy = Endpoint {
            friendly_name: "Speakers (DFX Audio Enhancer)".to_owned(),
            interface_name: "DFX Audio Enhancer".to_owned(),
            ..fxsound()
        };
        assert_eq!(legacy.virtual_kind(), Some(VirtualKind::FxSound));
        // Its recording endpoints are the driver's too, and so never a microphone to pick.
        let aux = endpoint(
            "{0.0.1.00000000}.{dc173c80}",
            IN,
            "Внутренний разъем  AUX (FxSound Audio Enhancer)",
            "Внутренний разъем  AUX",
            "FxSound Audio Enhancer",
        );
        assert_eq!(aux.virtual_kind(), Some(VirtualKind::FxSound));
        assert!(!is_offered(&aux));
        assert!(!is_offered(&fxsound()));
        assert!(is_offered(&speakers()));
        assert!(is_offered(&microphone()));
    }

    #[test]
    fn cables_are_found_by_their_adapter_and_nothing_else_is() {
        let [input, output] = cable();
        assert_eq!(input.virtual_kind(), Some(VirtualKind::VbCable));
        assert_eq!(output.virtual_kind(), Some(VirtualKind::VbCable));
        let [a, _] = cable_a();
        assert_eq!(a.virtual_kind(), Some(VirtualKind::VbAudioCable));
        let hifi = endpoint(
            "hifi",
            OUT,
            "Hi-Fi Cable Input (VB-Audio Hi-Fi Cable)",
            "Hi-Fi Cable Input",
            "VB-Audio Hi-Fi Cable",
        );
        assert_eq!(hifi.virtual_kind(), Some(VirtualKind::VbAudioCable));
        let line = endpoint(
            "line1",
            OUT,
            "Line 1 (Virtual Audio Cable)",
            "Line 1",
            "Virtual Audio Cable",
        );
        assert_eq!(line.virtual_kind(), Some(VirtualKind::VirtualAudioCable));
        // An adapter that says nothing: the name in brackets decides.
        let unnamed = Endpoint {
            interface_name: String::new(),
            ..cable()[0].clone()
        };
        assert_eq!(unnamed.virtual_kind(), Some(VirtualKind::VbCable));
        // VoiceMeeter is a mixer that plays what it is given to the speakers itself: as the
        // device FxSound captures, it would play everything twice. Nor is a streaming device
        // a cable.
        for (friendly, adapter) in [
            (
                "VoiceMeeter Input (VB-Audio VoiceMeeter VAIO)",
                "VB-Audio VoiceMeeter VAIO",
            ),
            (
                "Speakers (Steam Streaming Speakers)",
                "Steam Streaming Speakers",
            ),
            ("Динамики (Realtek(R) Audio)", "Realtek(R) Audio"),
        ] {
            let other = endpoint("x", OUT, friendly, "", adapter);
            assert_eq!(other.virtual_kind(), None, "{friendly}");
            assert!(is_offered(&other));
        }
    }

    #[test]
    fn the_output_lane_prefers_fxsound_s_driver_and_the_input_lane_takes_a_cable_of_its_own() {
        let [cable_in, cable_out] = cable();
        let all = [
            speakers(),
            fxsound(),
            microphone(),
            cable_in.clone(),
            cable_out.clone(),
        ];
        let plan = plan(&all, true, None);
        let output = plan.output.expect("FxSound's driver");
        assert_eq!(output.kind, VirtualKind::FxSound);
        assert_eq!(output.render, fxsound().id);
        assert_eq!(output.capture, None);
        assert_eq!(output.default_endpoint(OUT), Some(fxsound().id.as_str()));
        let input = plan.input.expect("the cable");
        assert_eq!(input.render, cable_in.id);
        assert_eq!(input.capture.as_deref(), Some(cable_out.id.as_str()));
        assert_eq!(input.default_endpoint(IN), Some(cable_out.id.as_str()));
    }

    #[test]
    fn one_cable_serves_the_output_lane_and_leaves_the_microphone_without_one() {
        let [cable_in, cable_out] = cable();
        let all = [speakers(), microphone(), cable_in.clone(), cable_out];
        let plan_on = plan(&all, true, None);
        assert_eq!(
            plan_on.output.as_ref().map(|d| d.render.as_str()),
            Some(cable_in.id.as_str())
        );
        assert_eq!(plan_on.input, None, "the only cable is the speakers'");
        // With the speakers' lane off, the microphone may have it.
        let plan_off = plan(&all, false, None);
        assert_eq!(
            plan_off.input.as_ref().map(|d| d.render.as_str()),
            Some(cable_in.id.as_str())
        );
        // A second cable is the microphone's whatever the speakers do.
        let [a_in, a_out] = cable_a();
        let more = [
            speakers(),
            cable()[0].clone(),
            cable()[1].clone(),
            a_in.clone(),
            a_out,
        ];
        let plan = plan(&more, true, None);
        assert_eq!(plan.output.map(|d| d.kind), Some(VirtualKind::VbCable));
        assert_eq!(plan.input.map(|d| d.render), Some(a_in.id));
    }

    #[test]
    fn the_microphone_keeps_its_cable_while_it_may_and_the_speakers_keep_their_preference() {
        let [c_in, c_out] = cable();
        let [a_in, a_out] = cable_a();
        let all = [speakers(), c_in.clone(), c_out, a_in.clone(), a_out];
        // The speakers take VB-Cable, the microphone Cable A.
        let on = plan(&all, true, None);
        assert_eq!(
            on.output.as_ref().map(|d| d.render.as_str()),
            Some(c_in.id.as_str())
        );
        assert_eq!(
            on.input.as_ref().map(|d| d.render.as_str()),
            Some(a_in.id.as_str())
        );
        // The speakers' lane is switched off and VB-Cable is free, and sorts first: the
        // microphone stays where it is.
        let off = plan(&all, false, Some(&a_in.id));
        assert_eq!(off.input.map(|d| d.render), Some(a_in.id.clone()));
        // Nothing kept, it would have taken VB-Cable.
        let fresh = plan(&all, false, None);
        assert_eq!(fresh.input.map(|d| d.render), Some(c_in.id.clone()));
        // A cable the speakers' lane takes back is not kept for the microphone.
        let taken = plan(&all, true, Some(&c_in.id));
        assert_eq!(taken.input.map(|d| d.render), Some(a_in.id.clone()));
        // Nor is one that has gone, or that is no cable at all.
        let gone = plan(&all[..3], false, Some(&a_in.id));
        assert_eq!(gone.input.map(|d| d.render), Some(c_in.id.clone()));
        let fx = [fxsound(), speakers()];
        assert_eq!(plan(&fx, false, Some(&fxsound().id)).input, None);
    }

    #[test]
    fn no_virtual_device_is_no_plan_and_a_cable_without_its_recording_side_is_no_microphone_cable()
    {
        assert_eq!(
            plan(&[speakers(), microphone()], true, None),
            VirtualPlan::default()
        );
        let [cable_in, _] = cable();
        let plan = plan(&[fxsound(), cable_in], true, None);
        assert!(plan.output.is_some());
        assert_eq!(plan.input, None, "CABLE Output is disabled");
    }

    #[test]
    fn a_virtual_audio_cable_line_is_twinned_with_the_line_of_its_own_name() {
        let line = |n: u8, direction| {
            endpoint(
                &format!("line{n}-{direction:?}"),
                direction,
                &format!("Line {n} (Virtual Audio Cable)"),
                &format!("Line {n}"),
                "Virtual Audio Cable",
            )
        };
        let all = [line(1, OUT), line(2, OUT), line(1, IN), line(2, IN)];
        assert_eq!(
            capture_twin(&all[1], &all).map(|e| e.id.as_str()),
            Some("line2-Input")
        );
        assert_eq!(
            capture_twin(&all[0], &all).map(|e| e.id.as_str()),
            Some("line1-Input")
        );
        assert_eq!(capture_twin(&fxsound(), &all), None);
        assert_eq!(capture_twin(&speakers(), &all), None);
    }

    #[test]
    fn the_choice_does_not_depend_on_the_order_the_endpoints_come_in() {
        let [a_in, a_out] = cable_a();
        let [c_in, c_out] = cable();
        let forward = [a_in.clone(), a_out.clone(), c_in.clone(), c_out.clone()];
        let backward = [c_out, c_in, a_out, a_in];
        assert_eq!(plan(&forward, true, None), plan(&backward, true, None));
        assert_eq!(
            plan(&forward, true, None).output.map(|d| d.kind),
            Some(VirtualKind::VbCable),
            "VB-Cable before Cable A"
        );
    }

    #[test]
    fn an_endpoint_reaches_the_gui_and_the_rules_under_its_id_and_its_friendly_name() {
        let device = speakers().to_audio_device(3, Some(&speakers().id));
        assert_eq!(device.id, 3);
        assert_eq!(device.name, speakers().id);
        assert_eq!(device.description, "Динамики (Realtek(R) Audio)");
        assert!(device.is_default);
        assert_eq!(device.direction, OUT);
        assert_eq!(device.form_factor, "speaker");
        assert!(!microphone().to_audio_device(4, None).is_default);
        assert_eq!(
            microphone().to_audio_device(4, None).form_factor,
            "microphone"
        );

        let info = speakers().to_device_info(3);
        assert_eq!(info.name, speakers().id);
        assert_eq!(info.description, "Динамики (Realtek(R) Audio)");
        assert_eq!(info.nick, "Динамики");
        assert!(info.available);
        assert_eq!(info.direction, OUT);
        // A store that names nothing still names the device by its ID.
        let bare = Endpoint {
            friendly_name: String::new(),
            description: String::new(),
            ..speakers()
        };
        assert_eq!(bare.label(), bare.id);
        assert_eq!(bare.to_device_info(1).nick, bare.id);
    }

    #[test]
    fn every_endpoint_form_factor_has_its_own_picture() {
        let with = |form, direction| Endpoint {
            form_factor: form,
            direction,
            ..speakers()
        };
        let keys: Vec<&str> = (0..=10)
            .map(|form| with(Some(form), OUT).form_factor().key())
            .collect();
        assert_eq!(
            keys,
            [
                "network",
                "speaker",
                "line-level",
                "headphone",
                "microphone",
                "headset",
                "handset",
                "digital-passthrough",
                "spdif",
                "hdmi",
                "unknown"
            ]
        );
        assert_eq!(with(None, OUT).form_factor(), FormFactor::Unknown);
        // An input that says speakers, or nothing, is a microphone.
        assert_eq!(with(Some(1), IN).form_factor(), FormFactor::Microphone);
        assert_eq!(with(None, IN).form_factor(), FormFactor::Microphone);
        assert_eq!(with(Some(5), IN).form_factor(), FormFactor::Headset);
    }

    #[test]
    fn one_bluetooth_headset_on_both_lanes_is_one_container_on_the_bluetooth_enumerators() {
        let headphones = Endpoint {
            enumerator: "BTHENUM".to_owned(),
            container: Some("{a0b1}".to_owned()),
            form_factor: Some(3),
            ..speakers()
        };
        let hands_free = Endpoint {
            enumerator: "BTHHFENUM".to_owned(),
            container: Some("{a0b1}".to_owned()),
            form_factor: Some(5),
            ..microphone()
        };
        assert!(one_headset(&headphones, &hands_free));
        let other = Endpoint {
            container: Some("{c2d3}".to_owned()),
            ..hands_free.clone()
        };
        assert!(!one_headset(&headphones, &other));
        // A USB headset switches no profile: its microphone leaves its music alone.
        let usb = Endpoint {
            enumerator: "USB".to_owned(),
            ..headphones.clone()
        };
        assert!(!one_headset(&usb, &hands_free));
        // Two endpoints that say no container are not one.
        let unknown = Endpoint {
            container: None,
            ..headphones
        };
        assert!(!one_headset(
            &unknown,
            &Endpoint {
                container: None,
                ..hands_free
            }
        ));
    }
}
