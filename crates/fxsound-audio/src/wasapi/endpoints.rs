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
//! translates, unlike the endpoints' own names ("Динамики", "Lautsprecher") — with the number
//! Windows puts in front of a second instance taken off first, so that "2- VB-Audio Virtual
//! Cable", a VB-Cable installed again beside the first, is a cable as much as the first is.
//!
//! A name proves nothing on a bus where the device names itself: a USB device's product string and
//! a Bluetooth device's advertised name are whatever its maker — or whoever built it — wrote there.
//! A gadget calling itself "FxSound Audio Enhancer" or "VB-Audio Virtual Cable" would otherwise be
//! made the default for every application, captured, and hidden from the device list, its sound
//! played on to the speakers so that nothing sounds wrong. No virtual device sits on such a bus —
//! a driver with no hardware is enumerated by the system (`ROOT`, or a software device) — so an
//! endpoint on one is never virtual, whatever it calls itself ([`Endpoint::on_hardware_bus`]).
//!
//! The output lane needs one playback endpoint: applications play into it, and the lane captures
//! it with WASAPI loopback. The input lane needs a cable: it plays the processed microphone into
//! the cable's playback side, and applications record it from the cable's recording side — its
//! twin ([`capture_twin`]). FxSound's driver has no such twin: its recording endpoints ("Microphone",
//! "Internal AUX", "MIDI") are the driver sample's own, not a loop of its speakers. So the input
//! lane needs a cable of its own, distinct from the output lane's ([`plan`]).
//!
//! What is FxSound's, in the sense `crate::is_fxsound_node` gives a node of the Linux engine —
//! never a device to pick — is every endpoint of FxSound's driver, and both sides of a cable a lane
//! stands on ([`fxsound_endpoints`], [`is_offered`]). Offered as an output, the device the output
//! lane captures would have it play into what it captures; a cable's recording side offered as a
//! microphone, the input lane record what it plays. A cable no lane stands on is offered like any
//! other device, as every endpoint but its driver's own is to the original
//! (`docs/spec/12-audio-io.md` §5, pass 2): a user who plays into "CABLE Input" for a streaming
//! program, having picked it in FxSound or made it their default while FxSound ran, has the output
//! lane play into it, and their default given back there — though the engine's device rules come
//! to such a cable only where the user chose it, never as a device just added, as the first one
//! there is, or for being the default Windows gave it as it was installed
//! (`super::Engine::device_infos`; `super`, "Whose default it is"). Nor does the input lane take a
//! cable the user has in use — a default of theirs on either side, or the device the output lane
//! plays to — while another will do ([`plan`]): the microphone mixed into what they route through
//! it would be theirs to hear.

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

/// The buses a physical device is on (`PKEY_Device_EnumeratorName`): USB, the HD Audio bus, PCI,
/// Intel's audio DSP bus, ACPI and FireWire — and Bluetooth's, which [`Endpoint::is_bluetooth`]
/// knows. No virtual device is enumerated on any of them (module docs, "The virtual devices").
const HARDWARE_BUSES: [&str; 6] = ["USB", "HDAUDIO", "PCI", "INTELAUDIO", "ACPI", "1394"];

/// `name` with the number Windows puts in front of a second device of the same name taken off:
/// "2- vb-audio virtual cable" is "vb-audio virtual cable". A name that only starts with a digit
/// is left as it is.
fn without_instance_number(name: &str) -> &str {
    let rest = name.trim_start_matches(|c: char| c.is_ascii_digit());
    if rest.len() == name.len() {
        return name;
    }
    rest.strip_prefix("- ").unwrap_or(name)
}

impl Endpoint {
    /// Which virtual device this endpoint is part of, if any (module docs, "The virtual devices").
    pub(crate) fn virtual_kind(&self) -> Option<VirtualKind> {
        if self.on_hardware_bus() {
            return None;
        }
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
        // Only here, where the name is matched: the adapter's name as Windows gives it, number
        // and all, is what tells one instance's twin from another's (`capture_twin`).
        let adapter = without_instance_number(adapter.trim());
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

    /// Whether the endpoint is on a bus physical devices are on ([`HARDWARE_BUSES`], Bluetooth's),
    /// where the names it carries may be the device's own say-so: never a virtual device's (module
    /// docs, "The virtual devices"). An endpoint whose store names no bus is not taken to be on
    /// one: a virtual device's may say nothing.
    pub(crate) fn on_hardware_bus(&self) -> bool {
        let bus = self.enumerator.trim().to_ascii_uppercase();
        self.is_bluetooth() || HARDWARE_BUSES.contains(&bus.as_str())
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

/// Whether an endpoint is offered to the user as a device to pick: every one that is not among
/// `ours`, FxSound's own endpoints ([`fxsound_endpoints`]; module docs).
pub(crate) fn is_offered(endpoint: &Endpoint, ours: &[String]) -> bool {
    !ours.contains(&endpoint.id)
}

/// FxSound's own endpoints among `endpoints`, never a device to pick: every endpoint of FxSound's
/// driver, and both sides of each of `lanes` — the virtual devices the lanes stand on (module
/// docs).
pub(crate) fn fxsound_endpoints(endpoints: &[Endpoint], lanes: &[&VirtualDevice]) -> Vec<String> {
    let mut ours: Vec<String> = endpoints
        .iter()
        .filter(|endpoint| endpoint.virtual_kind() == Some(VirtualKind::FxSound))
        .map(|endpoint| endpoint.id.clone())
        .collect();
    for device in lanes {
        ours.extend(device_of(&device.render, endpoints));
        ours.extend(device.capture.iter().cloned());
    }
    ours.sort();
    ours.dedup();
    ours
}

/// The endpoints of the virtual device `id` is a side of: a cable's playback side and its
/// recording side ([`capture_twin`]), whichever of the two `id` is — or `id` alone, for an endpoint
/// that is neither, or is not listed.
pub(crate) fn device_of(id: &str, endpoints: &[Endpoint]) -> Vec<String> {
    let Some(endpoint) = endpoints.iter().find(|endpoint| endpoint.id == id) else {
        return vec![id.to_owned()];
    };
    let mut sides = vec![id.to_owned()];
    match endpoint.direction {
        DeviceDirection::Output => {
            sides.extend(capture_twin(endpoint, endpoints).map(|twin| twin.id.clone()));
        }
        DeviceDirection::Input => sides.extend(
            endpoints
                .iter()
                .filter(|render| render.direction == DeviceDirection::Output)
                .filter(|render| capture_twin(render, endpoints).is_some_and(|twin| twin.id == id))
                .map(|render| render.id.clone()),
        ),
    }
    sides
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
    /// The input lane has no cable only because the ones it could take are in the user's use: a
    /// cable is there that the output lane does not stand on, and a default of theirs is on it, or
    /// the output lane plays into it ([`plan`]'s `in_use`). What the lane says then is not that
    /// there is none, but that it is in use (`crate::AudioError::VirtualInputDeviceInUse`).
    pub(crate) input_in_use: bool,
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
///
/// Nor does the input lane take a cable the user has in use (`in_use`, for either side's endpoint:
/// a default of theirs, the device the output lane plays to): it would mix the microphone into
/// what they send through it, and make its recording side the default of whatever records it. It
/// has none then, and the plan says why ([`VirtualPlan::input_in_use`]). A default Windows put on
/// a cable by itself is no use of the user's (`super`, "Whose default it is"): `in_use` leaves it
/// out, and the cable is the lane's to take.
pub(crate) fn plan(
    endpoints: &[Endpoint],
    output_enabled: bool,
    input_on: Option<&str>,
    in_use: impl Fn(&str) -> bool,
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
        kind.is_cable() && taken.as_deref() != Some(endpoint.id.as_str()) && !in_use(&endpoint.id)
    };
    let kept = renders
        .iter()
        .filter(free)
        .find(|(_, endpoint)| input_on == Some(endpoint.id.as_str()));
    let input = kept
        .into_iter()
        .chain(renders.iter().filter(free))
        .find_map(|&(kind, endpoint)| {
            capture_twin(endpoint, endpoints)
                .filter(|twin| !in_use(&twin.id))
                .map(|twin| VirtualDevice {
                    kind,
                    render: endpoint.id.clone(),
                    name: endpoint.label().to_owned(),
                    capture: Some(twin.id.clone()),
                })
        });
    // With none taken, any cable the output lane leaves and that has a recording side was passed
    // over for being in use.
    let input_in_use = input.is_none()
        && renders.iter().any(|&(kind, endpoint)| {
            kind.is_cable()
                && taken.as_deref() != Some(endpoint.id.as_str())
                && capture_twin(endpoint, endpoints).is_some()
        });
    VirtualPlan {
        output,
        input,
        input_in_use,
    }
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

    /// An endpoint of a driver with no hardware, which the system enumerates (`ROOT`), as
    /// FxSound's driver and the virtual cables are.
    pub(crate) fn virtual_endpoint(
        id: &str,
        direction: DeviceDirection,
        friendly: &str,
        description: &str,
        adapter: &str,
    ) -> Endpoint {
        Endpoint {
            enumerator: "ROOT".to_owned(),
            ..endpoint(id, direction, friendly, description, adapter)
        }
    }

    pub(crate) const OUT: DeviceDirection = DeviceDirection::Output;
    pub(crate) const IN: DeviceDirection = DeviceDirection::Input;

    /// Whether `endpoint` is offered while no lane stands on anything: only FxSound's driver is
    /// FxSound's then.
    fn offered_alone(endpoint: &Endpoint) -> bool {
        is_offered(
            endpoint,
            &fxsound_endpoints(std::slice::from_ref(endpoint), &[]),
        )
    }

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
        virtual_endpoint(
            "{0.0.0.00000000}.{73ca9fe5}",
            OUT,
            "FxSound Speakers (FxSound Audio Enhancer)",
            "FxSound Speakers",
            "FxSound Audio Enhancer",
        )
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
            virtual_endpoint(
                "{0.0.0.00000000}.{cable-in}",
                OUT,
                "CABLE Input (VB-Audio Virtual Cable)",
                "CABLE Input",
                "VB-Audio Virtual Cable",
            ),
            virtual_endpoint(
                "{0.0.1.00000000}.{cable-out}",
                IN,
                "CABLE Output (VB-Audio Virtual Cable)",
                "CABLE Output",
                "VB-Audio Virtual Cable",
            ),
        ]
    }

    /// A second VB-Cable, installed beside the first: Windows numbers its adapter.
    pub(crate) fn cable_2() -> [Endpoint; 2] {
        [
            virtual_endpoint(
                "{0.0.0.00000000}.{cable2-in}",
                OUT,
                "CABLE Input (2- VB-Audio Virtual Cable)",
                "CABLE Input",
                "2- VB-Audio Virtual Cable",
            ),
            virtual_endpoint(
                "{0.0.1.00000000}.{cable2-out}",
                IN,
                "CABLE Output (2- VB-Audio Virtual Cable)",
                "CABLE Output",
                "2- VB-Audio Virtual Cable",
            ),
        ]
    }

    pub(crate) fn cable_a() -> [Endpoint; 2] {
        [
            virtual_endpoint(
                "{0.0.0.00000000}.{cable-a-in}",
                OUT,
                "CABLE-A Input (VB-Audio Cable A)",
                "CABLE-A Input",
                "VB-Audio Cable A",
            ),
            virtual_endpoint(
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
        let aux = virtual_endpoint(
            "{0.0.1.00000000}.{dc173c80}",
            IN,
            "Внутренний разъем  AUX (FxSound Audio Enhancer)",
            "Внутренний разъем  AUX",
            "FxSound Audio Enhancer",
        );
        assert_eq!(aux.virtual_kind(), Some(VirtualKind::FxSound));
        assert!(!offered_alone(&aux));
        assert!(!offered_alone(&fxsound()));
        assert!(offered_alone(&speakers()));
        assert!(offered_alone(&microphone()));
    }

    #[test]
    fn cables_are_found_by_their_adapter_and_nothing_else_is() {
        let [input, output] = cable();
        assert_eq!(input.virtual_kind(), Some(VirtualKind::VbCable));
        assert_eq!(output.virtual_kind(), Some(VirtualKind::VbCable));
        let [a, _] = cable_a();
        assert_eq!(a.virtual_kind(), Some(VirtualKind::VbAudioCable));
        let hifi = virtual_endpoint(
            "hifi",
            OUT,
            "Hi-Fi Cable Input (VB-Audio Hi-Fi Cable)",
            "Hi-Fi Cable Input",
            "VB-Audio Hi-Fi Cable",
        );
        assert_eq!(hifi.virtual_kind(), Some(VirtualKind::VbAudioCable));
        let line = virtual_endpoint(
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
            // A device numbered by Windows is still only what its name says.
            ("Динамики (2- Realtek(R) Audio)", "2- Realtek(R) Audio"),
        ] {
            let other = virtual_endpoint("x", OUT, friendly, "", adapter);
            assert_eq!(other.virtual_kind(), None, "{friendly}");
            assert!(offered_alone(&other));
        }
    }

    #[test]
    fn a_cable_windows_numbered_as_a_second_instance_is_a_cable_too() {
        for (adapter, kind) in [
            ("2- VB-Audio Virtual Cable", VirtualKind::VbCable),
            ("3- VB-Audio Cable A", VirtualKind::VbAudioCable),
            ("2- VB-Audio Hi-Fi Cable", VirtualKind::VbAudioCable),
            ("2- Virtual Audio Cable", VirtualKind::VirtualAudioCable),
            ("12- Virtual Audio Cable", VirtualKind::VirtualAudioCable),
        ] {
            let numbered = virtual_endpoint(
                "numbered",
                OUT,
                &format!("Line 1 ({adapter})"),
                "Line 1",
                adapter,
            );
            assert_eq!(numbered.virtual_kind(), Some(kind), "{adapter}");
            // A device for the speakers' lane, and then FxSound's.
            let device = plan(std::slice::from_ref(&numbered), true, None, |_| false)
                .output
                .unwrap_or_else(|| panic!("{adapter}: a device for the speakers"));
            let ours = fxsound_endpoints(std::slice::from_ref(&numbered), &[&device]);
            assert!(!is_offered(&numbered, &ours), "{adapter}");
        }
        // An adapter that says nothing: the name in brackets, number and all.
        let unnamed = virtual_endpoint(
            "unnamed",
            OUT,
            "CABLE Input (2- VB-Audio Virtual Cable)",
            "CABLE Input",
            "",
        );
        assert_eq!(unnamed.virtual_kind(), Some(VirtualKind::VbCable));
        // A number with no dash after it is part of the name, not Windows' count.
        let other = virtual_endpoint("x", OUT, "x", "", "2 VB-Audio Virtual Cable");
        assert_eq!(other.virtual_kind(), None);
        assert_eq!(without_instance_number("2- name"), "name");
        assert_eq!(without_instance_number("name"), "name");
        assert_eq!(without_instance_number("2-name"), "2-name");
    }

    #[test]
    fn two_vb_cables_side_by_side_are_two_cables_each_with_its_own_recording_side() {
        let [c_in, c_out] = cable();
        let [c2_in, c2_out] = cable_2();
        let all = [
            speakers(),
            microphone(),
            c_in.clone(),
            c_out.clone(),
            c2_in.clone(),
            c2_out.clone(),
        ];
        for endpoint in &all[2..] {
            assert_eq!(endpoint.virtual_kind(), Some(VirtualKind::VbCable));
        }
        assert_eq!(capture_twin(&c_in, &all).map(|e| &e.id), Some(&c_out.id));
        assert_eq!(capture_twin(&c2_in, &all).map(|e| &e.id), Some(&c2_out.id));
        // One for each lane.
        let plan = plan(&all, true, None, |_| false);
        let output = plan.output.expect("a cable for the speakers");
        let input = plan.input.expect("another for the microphone");
        assert_ne!(output.render, input.render);
        let twin = |render: &str| {
            all.iter()
                .find(|endpoint| endpoint.id == render)
                .and_then(|endpoint| capture_twin(endpoint, &all))
                .map(|endpoint| endpoint.id.clone())
        };
        assert_eq!(input.capture, twin(&input.render));
    }

    #[test]
    fn a_device_on_a_hardware_bus_is_never_virtual_whatever_it_calls_itself() {
        // A USB gadget whose product string is FxSound's driver's name, or a cable's; a Bluetooth
        // device advertising one.
        for (bus, friendly, adapter) in [
            (
                "USB",
                "Speakers (FxSound Audio Enhancer)",
                "FxSound Audio Enhancer",
            ),
            (
                "USB",
                "Speakers (VB-Audio Virtual Cable)",
                "VB-Audio Virtual Cable",
            ),
            (
                "BTHENUM",
                "Headphones (VB-Audio Virtual Cable)",
                "VB-Audio Virtual Cable",
            ),
            (
                "BTHHFENUM",
                "Headset (Virtual Audio Cable)",
                "Virtual Audio Cable",
            ),
            (
                "HDAUDIO",
                "Speakers (DFX Audio Enhancer)",
                "DFX Audio Enhancer",
            ),
        ] {
            let spoof = Endpoint {
                enumerator: bus.to_owned(),
                ..virtual_endpoint("spoof", OUT, friendly, "Speakers", adapter)
            };
            assert!(spoof.on_hardware_bus(), "{bus}");
            assert_eq!(spoof.virtual_kind(), None, "{bus} {friendly}");
            assert!(offered_alone(&spoof), "{bus} {friendly}");
            // Nothing for a lane to stand on.
            assert_eq!(
                plan(&[speakers(), spoof.clone()], true, None, |_| false),
                VirtualPlan::default(),
                "{bus} {friendly}"
            );
        }
        // The system's own enumerators, and a store that names none, decide nothing.
        for bus in ["ROOT", "SWD", ""] {
            let device = Endpoint {
                enumerator: bus.to_owned(),
                ..fxsound()
            };
            assert!(!device.on_hardware_bus(), "{bus:?}");
            assert_eq!(device.virtual_kind(), Some(VirtualKind::FxSound), "{bus:?}");
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
        let plan = plan(&all, true, None, |_| false);
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
        let plan_on = plan(&all, true, None, |_| false);
        assert_eq!(
            plan_on.output.as_ref().map(|d| d.render.as_str()),
            Some(cable_in.id.as_str())
        );
        assert_eq!(plan_on.input, None, "the only cable is the speakers'");
        // With the speakers' lane off, the microphone may have it.
        let plan_off = plan(&all, false, None, |_| false);
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
        let plan = plan(&more, true, None, |_| false);
        assert_eq!(plan.output.map(|d| d.kind), Some(VirtualKind::VbCable));
        assert_eq!(plan.input.map(|d| d.render), Some(a_in.id));
    }

    #[test]
    fn the_microphone_keeps_its_cable_while_it_may_and_the_speakers_keep_their_preference() {
        let [c_in, c_out] = cable();
        let [a_in, a_out] = cable_a();
        let all = [speakers(), c_in.clone(), c_out, a_in.clone(), a_out];
        // The speakers take VB-Cable, the microphone Cable A.
        let on = plan(&all, true, None, |_| false);
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
        let off = plan(&all, false, Some(&a_in.id), |_| false);
        assert_eq!(off.input.map(|d| d.render), Some(a_in.id.clone()));
        // Nothing kept, it would have taken VB-Cable.
        let fresh = plan(&all, false, None, |_| false);
        assert_eq!(fresh.input.map(|d| d.render), Some(c_in.id.clone()));
        // A cable the speakers' lane takes back is not kept for the microphone.
        let taken = plan(&all, true, Some(&c_in.id), |_| false);
        assert_eq!(taken.input.map(|d| d.render), Some(a_in.id.clone()));
        // Nor is one that has gone, or that is no cable at all.
        let gone = plan(&all[..3], false, Some(&a_in.id), |_| false);
        assert_eq!(gone.input.map(|d| d.render), Some(c_in.id.clone()));
        let fx = [fxsound(), speakers()];
        assert_eq!(plan(&fx, false, Some(&fxsound().id), |_| false).input, None);
    }

    #[test]
    fn a_cable_no_lane_stands_on_is_a_device_like_any_other_and_one_a_lane_stands_on_is_fxsound_s()
    {
        let [c_in, c_out] = cable();
        let all = [
            speakers(),
            fxsound(),
            microphone(),
            c_in.clone(),
            c_out.clone(),
        ];
        // The speakers' lane on FxSound's driver, the microphone's on nothing: the cable is a
        // device the user may pick, as the original would have it.
        let plan = plan(&all, true, None, |_| false);
        let output = plan.output.clone().expect("FxSound's driver");
        let ours = fxsound_endpoints(&all, &[&output]);
        assert_eq!(ours, [fxsound().id]);
        for endpoint in [&c_in, &c_out, &speakers(), &microphone()] {
            assert!(is_offered(endpoint, &ours), "{}", endpoint.label());
        }
        assert!(!is_offered(&fxsound(), &ours));
        // The microphone's lane on it: both of its sides are FxSound's.
        let input = plan.input.expect("the cable");
        let ours = fxsound_endpoints(&all, &[&output, &input]);
        assert!(!is_offered(&c_in, &ours) && !is_offered(&c_out, &ours));
        assert!(is_offered(&speakers(), &ours));
        // The speakers' lane's cable, its recording side included, which nothing of FxSound's
        // records: offered as a microphone, it would hand the speakers' sound to a recorder.
        let only = [speakers(), microphone(), c_in.clone(), c_out.clone()];
        let output = plan_of(&only).output.expect("the cable");
        assert_eq!(output.capture, None);
        let ours = fxsound_endpoints(&only, &[&output]);
        assert!(!is_offered(&c_in, &ours) && !is_offered(&c_out, &ours));
        // Either side of a cable names the whole cable.
        assert_eq!(
            device_of(&c_out.id, &all),
            [c_out.id.clone(), c_in.id.clone()]
        );
        assert_eq!(
            device_of(&c_in.id, &all),
            [c_in.id.clone(), c_out.id.clone()]
        );
        assert_eq!(device_of(&speakers().id, &all), [speakers().id]);
        assert_eq!(device_of("gone", &all), ["gone"]);
    }

    /// The plan with the speakers' lane on, nothing kept and nothing in the user's use.
    fn plan_of(endpoints: &[Endpoint]) -> VirtualPlan {
        plan(endpoints, true, None, |_| false)
    }

    #[test]
    fn the_microphone_never_takes_a_cable_the_user_has_in_use() {
        let [c_in, c_out] = cable();
        let [a_in, a_out] = cable_a();
        let all = [
            speakers(),
            fxsound(),
            microphone(),
            c_in.clone(),
            c_out.clone(),
            a_in.clone(),
            a_out,
        ];
        assert_eq!(
            plan_of(&all).input.map(|d| d.render),
            Some(c_in.id.clone()),
            "VB-Cable first, when nobody uses it"
        );
        // "CABLE Input" the user's playback default, or what the speakers' lane plays to; "CABLE
        // Output" their recording default: Cable A is the microphone's.
        for used in [&c_in.id, &c_out.id] {
            let plan = plan(&all, true, None, |id| id == used.as_str());
            assert_eq!(
                plan.input.map(|d| d.render),
                Some(a_in.id.clone()),
                "{used}"
            );
        }
        // With no other cable the microphone has none, rather than the user's — not even the one it
        // stood on, once the user puts it to use — and the plan says it is in use, not missing.
        let one = [
            speakers(),
            fxsound(),
            microphone(),
            c_in.clone(),
            c_out.clone(),
        ];
        let used = plan(&one, true, None, |id| id == c_in.id);
        assert_eq!(used.input, None);
        assert!(used.input_in_use);
        let used = plan(&one, true, Some(&c_in.id), |id| id == c_out.id);
        assert_eq!(used.input, None);
        assert!(used.input_in_use);
        assert!(!plan_of(&one).input_in_use, "free, and the microphone's");
        // A machine with no cable, or whose only cable is the speakers' lane's, has none for the
        // microphone whatever the user uses.
        assert!(!plan(&one[..3], true, None, |_| true).input_in_use);
        let only_the_speakers = plan(&one[2..], true, None, |_| true);
        assert_eq!(only_the_speakers.input, None);
        assert!(!only_the_speakers.input_in_use);
        // The speakers' lane is held to nothing of the sort: FxSound's driver first, and with none,
        // the cable — a role the user put there is theirs, and stays; one Windows put there by
        // itself the lane adopts, and gives back to a real device (`super::defaults`).
        assert_eq!(
            plan(&one, true, None, |_| true).output.map(|d| d.kind),
            Some(VirtualKind::FxSound)
        );
        assert_eq!(
            plan(&one[2..], true, None, |_| true).output.map(|d| d.kind),
            Some(VirtualKind::VbCable)
        );
    }

    #[test]
    fn no_virtual_device_is_no_plan_and_a_cable_without_its_recording_side_is_no_microphone_cable()
    {
        assert_eq!(
            plan(&[speakers(), microphone()], true, None, |_| false),
            VirtualPlan::default()
        );
        let [cable_in, _] = cable();
        let plan = plan(&[fxsound(), cable_in], true, None, |_| false);
        assert!(plan.output.is_some());
        assert_eq!(plan.input, None, "CABLE Output is disabled");
    }

    #[test]
    fn a_virtual_audio_cable_line_is_twinned_with_the_line_of_its_own_name() {
        let line = |n: u8, direction| {
            virtual_endpoint(
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
        assert_eq!(
            plan(&forward, true, None, |_| false),
            plan(&backward, true, None, |_| false)
        );
        assert_eq!(
            plan(&forward, true, None, |_| false).output.map(|d| d.kind),
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
