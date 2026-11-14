//! Live checks against the machine the tests run on — read only, or a moment of digital silence:
//! the endpoints read and classified, a loopback capture opened and closed on the virtual device,
//! silence rendered to a real device, a lane's stream thread run between the two with nothing but
//! silence to render, the default-device policy's class and layout checked without writing
//! anything, the original FxSound found when it runs, and the probe. None of them changes a
//! default, starts the engine or plays a sound.
//!
//! A machine with nothing to check against — a CI runner has no audio endpoints, and may have no
//! audio service — passes each check, saying what it could not look at.
//!
//! The end-to-end check, which does change the default device and plays the system's sound through
//! FxSound, is [`end_to_end_on_this_machine`], `#[ignore]`d: it is run by hand, with the consent of
//! whoever owns the machine.

use std::time::{Duration, Instant};

use fxsound_core::DeviceDirection;

use super::com::{self, Apartment, CaptureStream, Enumerator, Packet, RenderStream, Source};
use super::endpoints::{self, Endpoint};
use super::format::StreamFormat;
use super::original;
use super::stream::{Outcome, Stream, StreamPlan};

/// The MTA for the test's thread, the enumerator, and every active endpoint; `None`, said aloud,
/// on a machine without an audio service.
fn system() -> Option<(Apartment, Enumerator, Vec<Endpoint>)> {
    let apartment = Apartment::join().expect("the test thread joins the MTA");
    let Ok(enumerator) = Enumerator::new() else {
        eprintln!("no MMDevice API here: nothing to check");
        return None;
    };
    let mut all = Vec::new();
    for direction in DeviceDirection::ALL {
        match enumerator.endpoints(direction) {
            Ok(endpoints) => all.extend(endpoints),
            Err(error) => {
                eprintln!(
                    "no {} endpoints here ({error}): nothing to check",
                    direction.key()
                );
                return None;
            }
        }
    }
    Some((apartment, enumerator, all))
}

#[test]
fn the_machine_s_endpoints_are_read_and_the_virtual_device_is_found_among_them() {
    let Some((_apartment, _enumerator, all)) = system() else {
        return;
    };
    for endpoint in &all {
        assert!(!endpoint.id.is_empty());
        eprintln!(
            "{:?} {:<50} {:?} bus {}",
            endpoint.direction,
            endpoint.label(),
            endpoint.virtual_kind(),
            endpoint.enumerator
        );
    }
    let plan = endpoints::plan(&all, true, None);
    // Where FxSound's driver is installed — the original FxSound's machine — it is the output
    // lane's device, whatever else is there.
    if all.iter().any(|endpoint| {
        endpoint.virtual_kind() == Some(endpoints::VirtualKind::FxSound)
            && endpoint.direction == DeviceDirection::Output
    }) {
        assert_eq!(
            plan.output.as_ref().map(|device| device.kind),
            Some(endpoints::VirtualKind::FxSound)
        );
    }
    eprintln!("plan: {plan:?}");
}

#[test]
fn a_loopback_capture_on_the_virtual_device_opens_reads_and_closes() {
    let Some((_apartment, enumerator, all)) = system() else {
        return;
    };
    let Some(device) = endpoints::plan(&all, true, None).output else {
        eprintln!("no virtual device here: nothing to capture");
        return;
    };
    let endpoint = enumerator
        .device(&device.render)
        .expect("the virtual device is there");
    let capture =
        CaptureStream::open(&endpoint, Source::Loopback).expect("a loopback stream opens");
    let format = capture.format();
    assert!((2..=8).contains(&format.channels));
    assert!(format.rate > 0);
    assert!(capture.period() > 0, "a packet is some frames long");
    capture.start().expect("it starts");
    let (mut packets, mut frames) = (0_usize, 0_usize);
    let until = Instant::now() + Duration::from_millis(200);
    while Instant::now() < until {
        let events: Vec<&com::WaitEvent> = capture.event().into_iter().collect();
        let _ = com::wait_any(&events, Duration::from_millis(20));
        capture
            .drain(|packet| {
                packets += 1;
                frames += match packet {
                    Packet::Samples(bytes) => bytes.len() / format.block_align(),
                    Packet::Silence(frames) => frames,
                };
            })
            .expect("packets are read");
    }
    capture.stop();
    eprintln!(
        "{}: {} Hz, {} ch, {} packets, {frames} frames in 200 ms",
        device.name, format.rate, format.channels, packets
    );
}

#[test]
fn digital_silence_renders_on_a_real_device_for_a_moment() {
    let Some((_apartment, enumerator, all)) = system() else {
        return;
    };
    let Some(output) = all.iter().find(|endpoint| {
        endpoint.direction == DeviceDirection::Output && endpoints::is_offered(endpoint)
    }) else {
        eprintln!("no real output here: nothing to render to");
        return;
    };
    let device = enumerator.device(&output.id).expect("the output is there");
    let format = StreamFormat {
        rate: 48_000,
        channels: 2,
        mask: 0x3,
    };
    let render = match RenderStream::open(&device, &format) {
        Ok(render) => render,
        // Held in exclusive mode by another application: nothing to learn here.
        Err(error) if com::is_device_in_use(&error) => return,
        Err(error) => panic!("{}: {error}", output.label()),
    };
    assert!(render.buffer() >= render.period());
    let mut written = 0;
    let first = render.period().min(render.buffer());
    render
        .write(first, |bytes| bytes.fill(0))
        .expect("silence is written");
    written += first;
    render.start().expect("it starts");
    let until = Instant::now() + Duration::from_millis(150);
    while Instant::now() < until {
        let _ = com::wait_any(&[render.event()], Duration::from_millis(20));
        let free = render.buffer() - render.padding().expect("the padding is read");
        render
            .write(free, |bytes| bytes.fill(0))
            .expect("silence is written");
        written += free;
    }
    render.stop();
    assert!(written > 0);
    eprintln!("{}: {written} frames of silence", output.label());
}

#[test]
fn a_lane_s_stream_runs_from_the_virtual_device_to_a_real_one_in_silence_and_stops() {
    let Some((_apartment, _enumerator, all)) = system() else {
        return;
    };
    let Some(device) = endpoints::plan(&all, true, None).output else {
        eprintln!("no virtual device here: nothing to capture");
        return;
    };
    let Some(output) = all.iter().find(|endpoint| {
        endpoint.direction == DeviceDirection::Output && endpoints::is_offered(endpoint)
    }) else {
        eprintln!("no real output here: nothing to render to");
        return;
    };
    let (lanes, _handover) = crate::lane_dsp::tests::lanes_for_tests();
    let (events, _ended) = crossbeam_channel::unbounded();
    let plan = StreamPlan {
        direction: DeviceDirection::Output,
        capture: device.render.clone(),
        render: output.id.clone(),
        generation: 1,
    };
    // Silent twice over, whatever plays into the virtual device meanwhile: the engine's mute, from
    // the first sample, and a volume of nothing after the chain. Nothing else is ever rendered.
    let stream = match Stream::start(plan, lanes.output, events, true, 0.0) {
        Ok(stream) => stream,
        Err((ended, error)) => {
            assert!(
                ended.dsp.is_some(),
                "the DSP comes back from a stream that failed"
            );
            eprintln!("{}: no stream ({error})", output.label());
            return;
        }
    };
    let opened = stream.opened;
    assert!((2..=8).contains(&opened.format.channels));
    assert!(opened.packet > 0 && opened.period > 0);
    assert!(opened.targets.ring >= 2 * opened.packet.max(opened.period));
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !stream.has_ended(),
        "the stream runs until it is asked to stop"
    );
    let started = Instant::now();
    let ended = stream.stop(Duration::ZERO);
    assert_eq!(ended.outcome, Outcome::Stopped);
    assert!(ended.dsp.is_some(), "and it hands the lane's DSP back");
    assert!(ended.left_behind.is_none());
    eprintln!(
        "{} → {}: {} ch @ {} Hz, packets of {}, periods of {}, ring at {} ± {}; stopped in {:?}, \
         {} frames processed",
        device.name,
        output.label(),
        opened.format.channels,
        opened.format.rate,
        opened.packet,
        opened.period,
        opened.targets.ring,
        opened.targets.band,
        started.elapsed(),
        ended.totals.frames
    );
}

#[test]
fn the_default_device_policy_is_the_class_and_the_layout_the_engine_declares() {
    let Some((_apartment, enumerator, all)) = system() else {
        return;
    };
    let policy = match com::PolicyConfig::new() {
        Ok(policy) => policy,
        Err(error) => {
            eprintln!("no default-device policy here ({error})");
            return;
        }
    };
    // Read, never written: an endpoint's mix format, through the policy's first slot and through
    // an audio client. The same answer says the class answers with the interface asked for and
    // its vtable is laid out as declared.
    let mut checked = 0;
    for endpoint in &all {
        let Ok(through_client) = enumerator
            .device(&endpoint.id)
            .and_then(|device| device.mix_format())
        else {
            continue;
        };
        let through_policy = policy
            .mix_format(&endpoint.id)
            .expect("the policy reads an endpoint's mix format");
        assert_eq!(through_policy, through_client, "{}", endpoint.label());
        checked += 1;
    }
    eprintln!("{checked} endpoints read the same through the policy");
}

#[test]
fn the_original_fxsound_is_found_whenever_it_runs_and_this_process_never_is() {
    let processes = com::processes();
    let own = com::own_pid();
    let own_image = com::process_image(own);
    assert!(own_image.is_some(), "this process reads its own path");
    assert!(processes.iter().any(|process| process.pid == own));
    let found = original::find(&processes, own, own_image.as_deref());
    let original_runs = processes.iter().any(|process| {
        process.exe.eq_ignore_ascii_case("fxsound.exe")
            && process
                .image
                .as_deref()
                .is_some_and(|image| image.to_lowercase().contains(r"\fxsound llc\"))
    });
    assert_eq!(found.is_some(), original_runs, "{found:?}");
    assert!(found.is_none_or(|process| process.pid != own));
}

#[test]
fn the_probe_reports_and_changes_no_default() {
    let Some((_apartment, enumerator, _)) = system() else {
        return;
    };
    let before: Vec<_> = DeviceDirection::ALL
        .into_iter()
        .map(|direction| enumerator.defaults(direction))
        .collect();
    let report = super::probe().expect("the probe runs");
    assert!(report.contains("read only"));
    let after: Vec<_> = DeviceDirection::ALL
        .into_iter()
        .map(|direction| enumerator.defaults(direction))
        .collect();
    assert_eq!(before, after);
}

/// The whole engine on this machine: started, playing the virtual device to the speakers with the
/// default on the virtual device, and shut down with every default given back.
///
/// **Not run by default: it changes the system's default device, and plays whatever the system
/// plays through FxSound for a few seconds.** To run it, with the consent of whoever owns the
/// machine:
///
/// 1. Quit the original FxSound (its tray icon, Exit) — while it runs the engine stands aside, and
///    the check only proves that.
/// 2. Have FxSound's driver or a virtual cable installed (`cargo run -p fxsound-audio --example
///    wasapi_probe` says which device the engine would use).
/// 3. `cargo test -p fxsound-audio --lib wasapi::live::end_to_end_on_this_machine -- --ignored
///    --nocapture`
///
/// The default devices are read before and compared after: the check fails if any role is not
/// given back.
#[test]
#[ignore = "changes the default device and plays the system's sound through FxSound; see its docs"]
fn end_to_end_on_this_machine() {
    use crate::ORIGINAL_FXSOUND_RUNNING;
    use fxsound_core::messages::AudioToUi;

    let Some((_apartment, enumerator, all)) = system() else {
        return;
    };
    let before: Vec<_> = DeviceDirection::ALL
        .into_iter()
        .map(|direction| enumerator.defaults(direction))
        .collect();
    let plan = endpoints::plan(&all, true, None);
    let handle = crate::AudioEngine::start().expect("the engine starts");
    let mut seen = Vec::new();
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        while let Some(message) = handle.try_recv() {
            eprintln!("{message:?}");
            seen.push(message);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // Named by itself, as every other call to `tr` names its key: the translations audit reads
    // this crate's calls, and a path it cannot follow is one it has to be told about.
    let original = seen.iter().any(|message| {
        matches!(message, AudioToUi::Warning { message, .. }
            if *message == fxsound_core::i18n::tr(ORIGINAL_FXSOUND_RUNNING))
    });
    let during = enumerator.defaults(DeviceDirection::Output);
    match (&plan.output, original) {
        (_, true) => assert_eq!(
            during, before[0],
            "the original FxSound's default is left alone"
        ),
        (Some(device), false) => {
            assert_eq!(
                during.main(),
                Some(device.render.as_str()),
                "the virtual device is the default"
            );
            assert!(
                seen.iter().any(|message| matches!(
                    message,
                    AudioToUi::Attached {
                        node_name: Some(_),
                        ..
                    }
                )),
                "the output lane attached"
            );
        }
        (None, false) => assert!(
            seen.iter()
                .any(|message| matches!(message, AudioToUi::Error { .. }))
        ),
    }
    handle.shutdown();
    let after: Vec<_> = DeviceDirection::ALL
        .into_iter()
        .map(|direction| enumerator.defaults(direction))
        .collect();
    assert_eq!(after, before, "every default given back");
}
