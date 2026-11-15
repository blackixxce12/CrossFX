//! What the Windows engine sees, without changing anything: every endpoint as it is classified,
//! the virtual device each lane would stand on, the formats the streams would run at, the defaults
//! and the original FxSound.
//!
//! Read only: endpoints are enumerated and their property stores read, a throw-away audio client
//! per endpoint is asked for its mix format and released uninitialised, the virtual device's volume
//! is read, the default-device policy is created and asked one endpoint's mix format — the check
//! that its class and layout are the ones `super::com` declares — FxSound's record of the defaults
//! it holds is read (`super::record`), and the process list is read. No stream is opened, no
//! default written, nothing registered.

use std::fmt::Write as _;

use fxsound_core::DeviceDirection;

use super::com;
use super::defaults::{Defaults, Role};
use super::endpoints::{self, Endpoint};
use super::format::{MixFormat, StreamFormat};
use super::original;
use crate::AudioError;
use crate::devices::{self, Preference, SelectionMemory};

/// A report of what the Windows engine sees (module docs), as text for a person to read.
///
/// Runs on a thread of its own, in the multithreaded apartment, so that it may be called from any
/// thread — a GUI's single-threaded one included.
///
/// # Errors
/// [`AudioError::WindowsAudioUnavailable`] when COM or the MMDevice API cannot be reached.
pub fn probe() -> Result<String, AudioError> {
    std::thread::Builder::new()
        .name("fxsound-probe".to_owned())
        .spawn(report)
        .map_err(|error| AudioError::WindowsAudioUnavailable(error.to_string()))?
        .join()
        .unwrap_or_else(|_| {
            Err(AudioError::WindowsAudioUnavailable(
                "the probe's thread panicked".to_owned(),
            ))
        })
}

fn report() -> Result<String, AudioError> {
    let _apartment = com::Apartment::join()
        .map_err(|error| AudioError::WindowsAudioUnavailable(error.message()))?;
    let enumerator = com::Enumerator::new()
        .map_err(|error| AudioError::WindowsAudioUnavailable(error.message()))?;
    let mut all: Vec<Endpoint> = Vec::new();
    for direction in DeviceDirection::ALL {
        all.extend(
            enumerator
                .endpoints(direction)
                .map_err(|error| AudioError::WindowsAudioUnavailable(error.message()))?,
        );
    }
    let defaults = [
        enumerator.defaults(DeviceDirection::Output),
        enumerator.defaults(DeviceDirection::Input),
    ];
    let mut out = String::new();
    let _ = writeln!(out, "FxSound WASAPI probe (read only: nothing is changed)");
    // As though both lanes were on: what each would stand on is FxSound's, every other endpoint a
    // device — a cable no lane would use among them.
    let plan = endpoints::plan(&all, true, None, |_| false);
    let lanes: Vec<_> = [plan.output.as_ref(), plan.input.as_ref()]
        .into_iter()
        .flatten()
        .collect();
    let fxsound_ids = endpoints::crossfx_endpoints(&all, &lanes);

    for (direction, defaults) in DeviceDirection::ALL.into_iter().zip(&defaults) {
        let _ = writeln!(
            out,
            "\n{} endpoints:",
            match direction {
                DeviceDirection::Output => "Playback",
                DeviceDirection::Input => "Recording",
            }
        );
        for endpoint in all
            .iter()
            .filter(|endpoint| endpoint.direction == direction)
        {
            describe(&mut out, &enumerator, endpoint, defaults, &fxsound_ids);
        }
        let _ = writeln!(
            out,
            "  defaults: console {}, multimedia {}, communications {}",
            name_of(&all, defaults.get(Role::Console)),
            name_of(&all, defaults.get(Role::Multimedia)),
            name_of(&all, defaults.get(Role::Communications)),
        );
    }

    let _ = writeln!(out, "\nVirtual devices:");
    match plan.output.as_ref() {
        Some(device) => {
            let _ = writeln!(out, "  output lane: {} ({:?})", device.name, device.kind);
            if let Ok(device_handle) = enumerator.device(&device.render) {
                if let Ok(mix) = device_handle.mix_format() {
                    match StreamFormat::for_loopback(&mix) {
                        Some(format) => {
                            let _ = writeln!(
                                out,
                                "    loopback at {} Hz, {} ch, layout {}{}",
                                format.rate,
                                format.channels,
                                format.layout().to_property_value(),
                                if format.is(&mix) {
                                    ", the mix format itself"
                                } else {
                                    ", converted by the engine"
                                }
                            );
                        }
                        None => {
                            let _ = writeln!(out, "    no format the chain runs at");
                        }
                    }
                }
                match device_handle.volume_levels() {
                    Ok((scalar, decibels, muted, hardware)) => {
                        let _ = writeln!(
                            out,
                            "    its volume: {:.0} % ({decibels:.1} dB){}, hardware support {hardware:#x}: \
                             the lane would play at {:.3}",
                            scalar * 100.0,
                            if muted { ", muted" } else { "" },
                            com::gain_of(muted, scalar, decibels)
                        );
                    }
                    Err(error) => {
                        let _ = writeln!(out, "    no volume control: {error}");
                    }
                }
            }
        }
        None => {
            let _ = writeln!(
                out,
                "  output lane: none: {} (install FxSound's driver or a virtual cable)",
                AudioError::NoVirtualDevice
            );
        }
    }
    match plan.input.as_ref() {
        Some(device) => {
            let _ = writeln!(
                out,
                "  input lane: plays into {} ({:?}); applications record from {}",
                device.name,
                device.kind,
                name_of(&all, device.capture.as_deref())
            );
        }
        None => {
            let _ = writeln!(
                out,
                "  input lane: none: {} (it needs a cable the speakers' lane does not stand on)",
                AudioError::NoVirtualInputDevice
            );
        }
    }

    // What the device rules would choose with nothing remembered and nothing ranked.
    let infos: Vec<_> = all
        .iter()
        .enumerate()
        .filter(|(_, endpoint)| endpoints::is_offered(endpoint, &fxsound_ids))
        .map(|(index, endpoint)| endpoint.to_device_info(index as u32))
        .collect();
    for (direction, defaults) in DeviceDirection::ALL.into_iter().zip(&defaults) {
        let ours = plan
            .get(direction)
            .and_then(|device| device.default_endpoint(direction))
            .unwrap_or_default();
        let choice = devices::choose_device(
            &infos,
            direction,
            ours,
            defaults.main(),
            &[],
            &SelectionMemory::default(),
            &Preference::default(),
        );
        let _ = writeln!(
            out,
            "  the {} lane's rules, with nothing remembered, choose: {}",
            direction.key(),
            match &choice {
                Ok(selection) => name_of(&all, Some(&selection.target)),
                Err(error) => error.to_string(),
            }
        );
    }

    let _ = writeln!(out, "\nThe default-device policy (IPolicyConfig):");
    match com::PolicyConfig::new() {
        Ok(policy) => {
            let _ = writeln!(out, "  the class answers with the interface");
            let checked = all.iter().find_map(|endpoint| {
                let through_policy = policy.mix_format(&endpoint.id).ok()?;
                let through_client = enumerator.device(&endpoint.id).ok()?.mix_format().ok()?;
                Some((endpoint, through_policy, through_client))
            });
            match checked {
                Some((endpoint, through_policy, through_client)) => {
                    let _ = writeln!(
                        out,
                        "  GetMixFormat through it on {}: {}; through IAudioClient: {}: {}",
                        endpoint.label(),
                        format_of(&through_policy),
                        format_of(&through_client),
                        if through_policy == through_client {
                            "the same, so the vtable is laid out as declared"
                        } else {
                            "different: the vtable is not what it is declared to be"
                        }
                    );
                }
                None => {
                    let _ = writeln!(out, "  no endpoint to check its layout on");
                }
            }
        }
        Err(error) => {
            let _ = writeln!(out, "  not there: {error}");
        }
    }

    let _ = writeln!(
        out,
        "\nFxSound's record of the defaults it holds (a run killed leaves it):"
    );
    let record = super::record::Record::of_user().read();
    for ((direction, endpoint), defaults) in record.claimed.iter().zip(&defaults) {
        let _ = writeln!(
            out,
            "  {} lane: {}",
            direction.key(),
            match endpoint {
                Some(endpoint) => format!(
                    "{}{}",
                    name_of(&all, Some(endpoint)),
                    if defaults.any_is(endpoint) {
                        ", still a default"
                    } else {
                        ", a default no more"
                    }
                ),
                None => "nothing".to_owned(),
            }
        );
    }
    let _ = writeln!(
        out,
        "\nThe defaults the user put on a virtual cable themselves, as the record keeps them:"
    );
    let mut none = true;
    for ((direction, users), defaults) in record.users.iter().zip(&defaults) {
        for role in Role::ALL {
            let Some(cable) = users.get(role) else {
                continue;
            };
            none = false;
            let _ = writeln!(
                out,
                "  {} {role:?}: {}{}",
                direction.key(),
                name_of(&all, Some(cable)),
                if defaults.get(role) == Some(cable) {
                    ", still the default"
                } else {
                    ", the default no more"
                }
            );
        }
    }
    if none {
        let _ = writeln!(out, "  none");
    }

    let _ = writeln!(out, "\nThe original FxSound:");
    let processes = com::processes();
    match original::find(&processes) {
        Some(process) => {
            let _ = writeln!(
                out,
                "  running: {} (process {}): the output lane would stand aside",
                process.image.as_deref().unwrap_or(&process.exe),
                process.pid
            );
        }
        None => {
            let _ = writeln!(out, "  not running");
        }
    }
    Ok(out)
}

/// One endpoint's lines of the report.
fn describe(
    out: &mut String,
    enumerator: &com::Enumerator,
    endpoint: &Endpoint,
    defaults: &Defaults,
    ours: &[String],
) {
    let roles: Vec<&str> = Role::ALL
        .into_iter()
        .filter(|&role| defaults.get(role) == Some(endpoint.id.as_str()))
        .map(|role| match role {
            Role::Console => "console",
            Role::Multimedia => "multimedia",
            Role::Communications => "communications",
        })
        .collect();
    let _ = writeln!(
        out,
        "  * {}{}",
        endpoint.label(),
        if roles.is_empty() {
            String::new()
        } else {
            format!("  [default: {}]", roles.join(", "))
        }
    );
    let _ = writeln!(out, "      id: {}", endpoint.id);
    let _ = writeln!(
        out,
        "      device {:?}, adapter {:?}, bus {:?}, form factor {}, container {}",
        endpoint.description,
        endpoint.interface_name,
        endpoint.enumerator,
        endpoint.form_factor().key(),
        endpoint.container.as_deref().unwrap_or("none")
    );
    match enumerator
        .device(&endpoint.id)
        .and_then(|device| device.mix_format())
    {
        Ok(mix) => {
            let _ = writeln!(out, "      mix format: {}", format_of(&mix));
        }
        Err(error) => {
            let _ = writeln!(out, "      mix format: unreadable ({error})");
        }
    }
    let _ = writeln!(
        out,
        "      {}",
        match endpoint.virtual_kind() {
            Some(kind) if !endpoints::is_offered(endpoint, ours) => {
                format!("virtual device ({kind:?}): FxSound's, never offered")
            }
            Some(kind) => format!("virtual device ({kind:?}), no lane's: offered as a device"),
            None => format!(
                "offered as an {}",
                match endpoint.direction {
                    DeviceDirection::Output => "output",
                    DeviceDirection::Input => "input",
                }
            ),
        }
    );
}

fn format_of(mix: &MixFormat) -> String {
    format!(
        "{} Hz, {} ch, {}-bit {}, layout {}",
        mix.rate,
        mix.channels,
        mix.bits,
        if mix.is_float32() { "float" } else { "integer" },
        mix.layout().to_property_value()
    )
}

/// An endpoint ID as the report names it: the endpoint's name, or the ID of one not listed.
fn name_of(all: &[Endpoint], id: Option<&str>) -> String {
    match id {
        None => "none".to_owned(),
        Some(id) => all
            .iter()
            .find(|endpoint| endpoint.id == id)
            .map_or_else(|| id.to_owned(), |endpoint| endpoint.label().to_owned()),
    }
}
