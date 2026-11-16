//! The ring between a lane's two streams, and what keeps it where it should be although the two
//! run on different clocks.
//!
//! # Two clocks
//!
//! On Linux both of a lane's nodes run in one graph cycle, on one clock, and the ring between them
//! holds a constant fill (`docs/spec/12-audio-io.md` §23). Here they do not: the output lane
//! captures the virtual device on that device's clock and plays to the speakers on theirs, and the
//! input lane records the microphone on its clock and plays into a cable on the cable's. Two
//! crystals a hundred parts per million apart move the fill by five frames a second at 48 kHz —
//! an underrun every few minutes, or a latency that grows without end. The original had the same
//! two clocks and answered with the drain-at-quarter rule and a silent truncation that dropped
//! audio (`sndDevicesDoCapture.cpp:262`, `sndDevicesDoPlayback.cpp:99-100`).
//!
//! Here [`DriftControl`] watches the fill each time the render stream is written, smoothed over
//! about a second, and when it has wandered more than [`Targets::band`] from [`Targets::ring`] it
//! has the next write take one frame more from the ring, or one frame fewer, than it plays. The
//! frame is not dropped or repeated outright: [`FrameRing::pop_into`] cross-fades the block over
//! [`CROSSFADE_FRAMES`] around the seam, so a correction is a slip of one sample spread over two
//! thirds of a millisecond — far below anything a listener hears, and at 100 ppm about five of them
//! a second.
//!
//! The band is wide on purpose. The fill read at each write moves with the phase between the two
//! streams' packets as well as with the clocks: the capture side arrives a packet at a time, the
//! render side leaves a period at a time, and the phase between the two drifts slowly — at 100 ppm
//! it takes a minute and a half to go round once — so no smoothing short enough to follow the
//! clocks averages it away. A band wider than a packet lets the phase come and go untouched and
//! answers only to what accumulates.
//!
//! # A source that stops
//!
//! A loopback stream delivers no packets at all while nothing plays into the virtual device: not
//! silent ones — none. So the capture side of the output lane does not end a sound with silence,
//! as a PipeWire sink's graph does; it simply stops. Once it has delivered nothing for a while
//! ([`quiet_after`]: two packets, longer than a packet comes late, shorter than the ring's cushion)
//! the stream thread takes it that the source has stopped, and runs what the ring lacks through
//! the chain as silence ([`silence_lacking`]), so that the ring stands at its target after every
//! write, the chain rings out rather than being cut off, and nothing is counted as an underrun.
//! A source that comes back finds the ring at its fill, with nothing to prime again.
//!
//! # Real-time rules
//!
//! Everything here runs on the lane's stream thread under MMCSS: the ring is allocated once, when
//! the stream is built, and nothing after that allocates, locks or panics.

use std::time::Duration;

/// How many frames a correction cross-fades over: two thirds of a millisecond at 48 kHz.
pub(crate) const CROSSFADE_FRAMES: usize = 32;

/// How many packets' worth of time the capture side may deliver nothing before its source is
/// taken to have stopped rather than to be late (module docs, "A source that stops").
pub(crate) const QUIET_PACKETS: u32 = 2;

/// The shortest such wait, whatever the packets.
pub(crate) const QUIET_FLOOR: Duration = Duration::from_millis(20);

/// How long the capture side of a stream whose packets are `packet` frames long at `rate` may
/// deliver nothing before its source is taken to have stopped: [`QUIET_PACKETS`] packets, and
/// never less than [`QUIET_FLOOR`].
pub(crate) fn quiet_after(packet: usize, rate: u32) -> Duration {
    let packet_nanos = (packet as u64).saturating_mul(1_000_000_000) / u64::from(rate.max(1));
    Duration::from_nanos(packet_nanos.saturating_mul(u64::from(QUIET_PACKETS))).max(QUIET_FLOOR)
}

/// How much silence the ring lacks, once its source has stopped, for the render side's next write
/// of `want` frames to leave it at its target ([`Targets::ring`]) — or, before it has been primed
/// (a sound shorter than the cushion), to reach the fill it is primed at ([`Targets::prime`]).
/// Nothing when what it holds is enough already.
pub(crate) fn silence_lacking(fill: usize, want: usize, targets: &Targets, primed: bool) -> usize {
    let goal = if primed {
        want + targets.ring
    } else {
        targets.prime()
    };
    goal.saturating_sub(fill)
}

/// Which way the next write from the ring corrects for the two clocks ([`DriftControl`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Correction {
    /// Play what is taken.
    None,
    /// The ring is filling: take one frame more than is played.
    Skip,
    /// The ring is emptying: take one frame fewer than is played.
    Stretch,
}

/// The fills a lane's ring and its render buffer are held at, from the buffer setting's quantum
/// and the two streams' own packet sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Targets {
    /// The fill the ring is primed to before anything is played, and held at afterwards.
    pub(crate) ring: usize,
    /// How far the smoothed fill may wander from [`Self::ring`] before [`DriftControl`] steps in.
    pub(crate) band: usize,
    /// How much the render stream's own buffer is kept holding: a period and a half, so a write
    /// that comes late by most of a period still finds the device something to play.
    pub(crate) padding: usize,
}

impl Targets {
    /// The targets for a lane whose buffer setting asks for blocks of `quantum` frames, whose
    /// capture stream delivers packets of `packet` frames and whose render stream wakes every
    /// `period` frames.
    ///
    /// The block is the largest of the three, and the ring is primed to two of them: a block and a
    /// half, the Linux ring's cushion (`crate::engine`, `TARGET_FILL_HALF_QUANTA`), and the half
    /// block more the drift band takes out of it. With the band at three quarters of a block, the
    /// fill can sit at the band's edge with the packets' phase against it and still hold three
    /// quarters of a block.
    pub(crate) fn new(quantum: usize, packet: usize, period: usize) -> Self {
        let block = quantum.max(packet).max(period).max(1);
        Self {
            ring: block * 2,
            band: block * 3 / 4,
            padding: period.max(1) * 3 / 2,
        }
    }

    /// What the ring is filled to before the render stream plays anything: the ring's own target
    /// and the render buffer's, which the first write fills from it — so that once that write is
    /// made the ring stands at its target, rather than a buffer's worth short of it for the drift
    /// control to make up a frame at a time.
    pub(crate) const fn prime(&self) -> usize {
        self.ring + self.padding
    }
}

/// The buffer setting's quantum at `rate`: [`crate::DEFAULT_QUANTUM_FRAMES`] at 48 kHz — the
/// `Normal` row of `docs/spec/12-audio-io.md` §24, ≈10.7 ms — and the same length of time at any
/// other rate.
pub(crate) fn quantum_frames(rate: u32) -> usize {
    let frames = u64::from(crate::DEFAULT_QUANTUM_FRAMES) * u64::from(rate)
        / u64::from(crate::DEFAULT_SAMPLE_RATE);
    usize::try_from(frames).map_or(crate::MAX_QUANTUM_FRAMES, |frames| {
        frames.clamp(1, crate::MAX_QUANTUM_FRAMES)
    })
}

/// A ring of interleaved `f32` frames between a lane's capture side and its render side.
///
/// Both ends are on the lane's stream thread, so unlike the Linux ring (`crate::engine`'s
/// `SampleRing`, whose two ends are two nodes' callbacks) it needs no atomics: a plain buffer and
/// two free-running frame counters. Allocated once, never resized.
pub(crate) struct FrameRing {
    samples: Box<[f32]>,
    channels: usize,
    capacity: usize,
    read: usize,
    write: usize,
}

impl FrameRing {
    /// A ring of at least `capacity` frames of `channels` channels, rounded up to a power of two.
    pub(crate) fn new(channels: usize, capacity: usize) -> Self {
        let channels = channels.max(1);
        let capacity = capacity.max(1).next_power_of_two();
        Self {
            samples: vec![0.0; capacity * channels].into_boxed_slice(),
            channels,
            capacity,
            read: 0,
            write: 0,
        }
    }

    /// Frames waiting to be played.
    pub(crate) const fn len(&self) -> usize {
        self.write.wrapping_sub(self.read)
    }

    /// Whether nothing waits to be played.
    #[cfg(test)]
    pub(crate) const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Forget everything waiting: a lane that went idle starts its next sound from an empty ring.
    pub(crate) const fn clear(&mut self) {
        self.read = self.write;
    }

    /// Prime: with at least `target` frames waiting, drop the oldest of them until no more than
    /// `slack` frames beyond `target` are left, and say so.
    ///
    /// The fill a ring is primed at is the fill it keeps for as long as the two clocks agree, so
    /// a ring primed again after it ran dry (`slack` 0) is set to its target here, before anything
    /// is played, rather than left at whatever the last packet happened to bring — up to a packet
    /// more, which the drift control would then take back a frame at a time. Nothing is fading
    /// there: the sound stopped short, and starts again where it is.
    ///
    /// The start of a sound is another matter. The chain fades it in from silence, from the first
    /// frame of its first packet on, and the oldest frames are that fade's beginning: dropped, the
    /// device's first sample after the silence would come from part-way up it — a step of about
    /// −18 dB at 48 kHz with 10 ms periods, on every stream start and every sound after an idle
    /// second. So it keeps what it has up to the drift band (`slack` = [`Targets::band`]), which
    /// the fill may sit in anyway: a packet's worth over the target, at most, with the thread
    /// woken on time, and nothing the drift control has to take back.
    pub(crate) fn prime(&mut self, target: usize, slack: usize) -> bool {
        let Some(excess) = self.len().checked_sub(target) else {
            return false;
        };
        self.read = self.read.wrapping_add(excess.saturating_sub(slack));
        true
    }

    /// The samples of the frame `index` frames after the read cursor.
    fn frame(&self, index: usize) -> &[f32] {
        let at = (self.read.wrapping_add(index) & (self.capacity - 1)) * self.channels;
        self.samples.get(at..at + self.channels).unwrap_or(&[])
    }

    /// Append whole frames of interleaved samples. Returns how many frames did not fit, which are
    /// dropped: a render side that has stalled must not stall the capture side too.
    pub(crate) fn push(&mut self, samples: &[f32]) -> usize {
        let frames = samples.len() / self.channels;
        let take = frames.min(self.capacity - self.len());
        for (offset, frame) in samples.chunks_exact(self.channels).take(take).enumerate() {
            let at = (self.write.wrapping_add(offset) & (self.capacity - 1)) * self.channels;
            if let Some(slot) = self.samples.get_mut(at..at + self.channels) {
                slot.copy_from_slice(frame);
            }
        }
        self.write = self.write.wrapping_add(take);
        frames - take
    }

    /// Write `frames` frames into `out` — little-endian `f32`, interleaved, as the render buffer
    /// takes them — from the ring, corrected as `correction` says. Returns how many frames had to
    /// be silence because the ring ran dry.
    ///
    /// A correction is made only when the ring holds every frame it takes and the block is long
    /// enough to cross-fade in ([`CROSSFADE_FRAMES`]); otherwise the block is played as taken.
    /// [`Correction::Skip`] plays frames `0..s` as they are, then over the cross-fade moves from
    /// each frame to the one after it, and plays the rest one frame late: one frame more taken
    /// than played. [`Correction::Stretch`] does the same towards the frame before: one frame
    /// fewer. Either way every output frame lies between two neighbours of the input, so the wave
    /// is continuous across the seam.
    pub(crate) fn pop_into(
        &mut self,
        out: &mut [u8],
        frames: usize,
        correction: Correction,
    ) -> usize {
        let channels = self.channels;
        let (words, _) = out.as_chunks_mut::<{ size_of::<f32>() }>();
        let frames = frames.min(words.len() / channels);
        let available = self.len();
        let fits = frames >= CROSSFADE_FRAMES + 2;
        let correction = match correction {
            Correction::Skip if fits && available > frames => Correction::Skip,
            Correction::Stretch if fits && available + 1 >= frames => Correction::Stretch,
            _ => Correction::None,
        };
        let start = frames.saturating_sub(CROSSFADE_FRAMES) / 2;
        let seam = start..start + CROSSFADE_FRAMES;
        // How far output frame `j`, inside the seam, has moved towards the next input frame.
        let along = |j: usize| (j - start + 1) as f32 / (CROSSFADE_FRAMES + 1) as f32;
        // For output frame `j`: the two input frames it lies between, and how far towards the
        // second. `start` is at least one whenever a correction is made (`fits`), so the frame
        // before `j` is there for a stretch.
        let source = |j: usize| -> (usize, usize, f32) {
            match correction {
                Correction::None => (j, j, 0.0),
                Correction::Skip | Correction::Stretch if j < start => (j, j, 0.0),
                Correction::Skip if seam.contains(&j) => (j, j + 1, along(j)),
                Correction::Skip => (j + 1, j + 1, 0.0),
                Correction::Stretch if seam.contains(&j) => (j, j - 1, along(j)),
                Correction::Stretch => (j - 1, j - 1, 0.0),
            }
        };
        let played = match correction {
            Correction::None => frames.min(available),
            Correction::Skip | Correction::Stretch => frames,
        };
        for (j, frame) in words.chunks_exact_mut(channels).take(frames).enumerate() {
            if j >= played {
                for word in frame {
                    *word = 0.0_f32.to_le_bytes();
                }
                continue;
            }
            let (from, to, along) = source(j);
            let (a, b) = (self.frame(from), self.frame(to));
            for (channel, word) in frame.iter_mut().enumerate() {
                let a = a.get(channel).copied().unwrap_or(0.0);
                let b = b.get(channel).copied().unwrap_or(0.0);
                *word = (a + (b - a) * along).to_le_bytes();
            }
        }
        let taken = match correction {
            Correction::None => played,
            Correction::Skip => frames + 1,
            Correction::Stretch => frames - 1,
        };
        self.read = self.read.wrapping_add(taken);
        frames - played
    }
}

/// Holds a ring's fill near its target against the drift between two clocks (module docs).
#[derive(Debug, Clone)]
pub(crate) struct DriftControl {
    target: f64,
    band: f64,
    /// The fill, smoothed over about a second of writes; `None` until the first since the ring
    /// was primed.
    smoothed: Option<f64>,
    /// Corrections made since the stream was built, for the log.
    corrections: u64,
}

impl DriftControl {
    /// The weight of one reading: with a render stream that wakes every 10 ms, a time constant of
    /// a second.
    const WEIGHT: f64 = 0.01;

    pub(crate) fn new(targets: &Targets) -> Self {
        Self {
            target: targets.ring as f64,
            band: targets.band as f64,
            smoothed: None,
            corrections: 0,
        }
    }

    /// Start smoothing afresh: the ring has just been primed again.
    pub(crate) const fn reset(&mut self) {
        self.smoothed = None;
    }

    /// The ring's fill right after a write to the render stream.
    pub(crate) fn observe(&mut self, fill: usize) {
        let fill = fill as f64;
        self.smoothed = Some(match self.smoothed {
            None => fill,
            Some(smoothed) => smoothed + Self::WEIGHT * (fill - smoothed),
        });
    }

    /// How the next write corrects. The correction is counted into the smoothed fill at once,
    /// rather than left for the smoothing to discover a second later, so the control does not
    /// keep correcting for a fill it has already corrected.
    pub(crate) fn correction(&mut self) -> Correction {
        let Some(smoothed) = self.smoothed.as_mut() else {
            return Correction::None;
        };
        if *smoothed > self.target + self.band {
            *smoothed -= 1.0;
            self.corrections += 1;
            Correction::Skip
        } else if *smoothed < self.target - self.band {
            *smoothed += 1.0;
            self.corrections += 1;
            Correction::Stretch
        } else {
            Correction::None
        }
    }

    /// Corrections made so far.
    pub(crate) const fn corrections(&self) -> u64 {
        self.corrections
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples_of(bytes: &[u8]) -> Vec<f32> {
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| f32::from_le_bytes(*word))
            .collect()
    }

    fn pop(ring: &mut FrameRing, frames: usize, correction: Correction) -> (Vec<f32>, usize) {
        let mut out = vec![0xAA_u8; frames * ring.channels * 4];
        let short = ring.pop_into(&mut out, frames, correction);
        (samples_of(&out), short)
    }

    #[test]
    fn frames_come_out_in_the_order_they_went_in_across_the_wrap() {
        let mut ring = FrameRing::new(2, 8);
        for round in 0..5 {
            let block: Vec<f32> = (0..6).map(|i| (round * 10 + i) as f32).collect();
            assert_eq!(ring.push(&block), 0);
            assert_eq!(ring.len(), 3);
            let (out, short) = pop(&mut ring, 3, Correction::None);
            assert_eq!(short, 0);
            assert_eq!(out, block, "round {round}");
            assert!(ring.is_empty());
        }
    }

    #[test]
    fn what_does_not_fit_is_dropped_and_what_is_missing_is_silence() {
        let mut ring = FrameRing::new(1, 4);
        assert_eq!(
            ring.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
            2,
            "two frames past the end"
        );
        assert_eq!(ring.len(), 4);
        let (out, short) = pop(&mut ring, 6, Correction::None);
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0, 0.0, 0.0]);
        assert_eq!(short, 2);
        // A partial frame is not half of one.
        let mut stereo = FrameRing::new(2, 4);
        assert_eq!(stereo.push(&[1.0, 2.0, 3.0]), 0);
        assert_eq!(stereo.len(), 1);
        stereo.clear();
        assert!(stereo.is_empty());
    }

    /// A ramp through a corrected block: the seam must stay inside the ramp's own steps, which is
    /// what "no click" means for the slip of one frame.
    #[test]
    fn a_correction_takes_one_frame_more_or_fewer_and_the_wave_stays_continuous() {
        let frames = 480;
        for (correction, taken) in [
            (Correction::Skip, frames + 1),
            (Correction::Stretch, frames - 1),
            (Correction::None, frames),
        ] {
            let mut ring = FrameRing::new(1, 4096);
            let ramp: Vec<f32> = (0..2 * frames).map(|i| i as f32).collect();
            ring.push(&ramp);
            let (out, short) = pop(&mut ring, frames, correction);
            assert_eq!(short, 0);
            assert_eq!(ring.len(), 2 * frames - taken, "{correction:?}");
            assert_eq!(out[0], 0.0, "starts where the ring stood");
            let steps: Vec<f32> = out.windows(2).map(|pair| pair[1] - pair[0]).collect();
            let (low, high) = steps
                .iter()
                .fold((f32::MAX, f32::MIN), |(l, h), &s| (l.min(s), h.max(s)));
            let seam = 1.0 / (CROSSFADE_FRAMES + 1) as f32;
            assert!(
                low >= 1.0 - seam - 1e-4 && high <= 1.0 + seam + 1e-4,
                "{correction:?}: steps between {low} and {high}"
            );
            // And the next block carries on from the frame after the last one taken.
            let (next, _) = pop(&mut ring, 1, Correction::None);
            assert_eq!(next[0], taken as f32, "{correction:?}");
        }
    }

    #[test]
    fn a_correction_the_ring_cannot_afford_or_a_block_too_short_for_is_not_made() {
        let mut ring = FrameRing::new(1, 64);
        ring.push(&[0.5; 40]);
        let (_, short) = pop(&mut ring, 40, Correction::Skip);
        assert_eq!(short, 0);
        assert!(
            ring.is_empty(),
            "taken as it was: the 41st frame is not there"
        );

        ring.push(&[0.5; 20]);
        let (_, short) = pop(&mut ring, 20, Correction::Stretch);
        assert_eq!(short, 0, "too short to cross-fade, so played as taken");
        assert!(ring.is_empty());
    }

    #[test]
    fn the_start_of_a_sound_is_primed_whole_and_a_ring_that_ran_dry_is_set_to_its_target() {
        // FxSound Speakers to the Realtek speakers on this machine: 48 kHz, 10 ms packets and
        // periods. Primed at 1744 frames; four packets bring 1920.
        let targets = Targets::new(512, 480, 480);
        let prime = targets.prime();
        assert_eq!(prime, 1744);
        // A sound the chain fades in over 30 ms, from its first frame on.
        let fade: Vec<f32> = (0..4 * 480).map(|i| (i as f32 / 1440.0).min(1.0)).collect();
        assert!(
            !FrameRing::new(1, 4096).prime(prime, targets.band),
            "nothing to play yet"
        );
        let mut start = FrameRing::new(1, 4096);
        start.push(&fade);
        assert!(start.prime(prime, targets.band));
        assert_eq!(start.len(), fade.len(), "the start of the fade is kept");
        let (first, short) = pop(&mut start, targets.padding, Correction::None);
        assert_eq!(short, 0);
        assert_eq!(
            first[0], 0.0,
            "the device's first sample is the fade's first"
        );
        // What the start of a sound keeps is inside the band: nothing for the drift control.
        let mut drift = DriftControl::new(&targets);
        drift.observe(start.len());
        assert_eq!(start.len(), targets.ring + 176);
        assert_eq!(drift.correction(), Correction::None);

        // A ring primed again after it ran dry is set to its target: nothing fades there.
        let mut dry = FrameRing::new(1, 4096);
        dry.push(&fade);
        assert!(dry.prime(prime, 0));
        assert_eq!(dry.len(), prime);
        let (first, _) = pop(&mut dry, 1, Correction::None);
        assert_eq!(first[0], fade[176]);

        // A start that finds far more than the band — a thread woken late — keeps the band's
        // worth beyond the target, and no more.
        let mut late = FrameRing::new(1, 4096);
        late.push(&(0..3000).map(|i| i as f32).collect::<Vec<_>>());
        assert!(late.prime(prime, targets.band));
        assert_eq!(late.len(), prime + targets.band);
    }

    #[test]
    fn the_targets_hold_two_blocks_with_a_band_inside_them() {
        let targets = Targets::new(512, 480, 480);
        assert_eq!(targets.ring, 1024);
        assert_eq!(targets.band, 384);
        assert_eq!(targets.padding, 720);
        // A device with long periods sets the block.
        let bluetooth = Targets::new(512, 480, 1920);
        assert_eq!(bluetooth.ring, 3840);
        // The buffer setting's quantum is the same length of time at any rate.
        assert_eq!(quantum_frames(48_000), 512);
        assert_eq!(quantum_frames(96_000), 1024);
        assert_eq!(quantum_frames(44_100), 470);
        assert_eq!(quantum_frames(1_000_000), crate::MAX_QUANTUM_FRAMES);
    }

    #[test]
    fn the_control_steps_in_only_outside_its_band_and_counts_what_it_did() {
        let targets = Targets::new(100, 100, 100);
        let mut drift = DriftControl::new(&targets);
        assert_eq!(drift.correction(), Correction::None, "nothing seen yet");
        drift.observe(targets.ring + targets.band);
        assert_eq!(drift.correction(), Correction::None, "at the edge, inside");
        drift.observe(targets.ring + 10 * targets.band);
        assert_eq!(drift.correction(), Correction::Skip);
        drift.reset();
        drift.observe(0);
        assert_eq!(drift.correction(), Correction::Stretch);
        assert_eq!(drift.corrections(), 2);
    }

    /// What one simulated run of a lane saw, from its first second on.
    struct Run {
        corrections: u64,
        underruns: usize,
        device_starved: usize,
        lowest: usize,
        highest: usize,
        steepest: f32,
        /// Frames of silence made up for a source that had stopped.
        synthesized: usize,
    }

    /// The test signal: a 100 Hz triangle at 48 kHz, full scale. Its steepest step between two
    /// samples is [`TRIANGLE_SLOPE`], and a correction that clicked would step further.
    fn triangle(n: usize) -> f32 {
        let phase = (n % 480) as f32 / 240.0;
        (phase - 1.0).abs() * 2.0 - 1.0
    }

    const TRIANGLE_SLOPE: f32 = 2.0 / 240.0;

    /// A lane's capture side delivering `packet` frames on a clock `ppm` parts per million fast
    /// (negative: slow) against a render side that wakes every `period` frames and keeps its buffer
    /// at the target, for `seconds`, through the ring and the drift control as the stream thread
    /// drives them (`super::super::stream`). During `gap`, from its first second to its second,
    /// the capture side delivers nothing, as a loopback stream does once its application stops.
    fn simulate(
        ppm: f64,
        packet: usize,
        period: usize,
        seconds: f64,
        gap: Option<(f64, f64)>,
    ) -> Run {
        const RATE: f64 = 48_000.0;
        let targets = Targets::new(quantum_frames(48_000), packet, period);
        let mut ring = FrameRing::new(1, crate::RING_CAPACITY_FRAMES);
        let mut drift = DriftControl::new(&targets);
        let capture_every = packet as f64 / (RATE * (1.0 + ppm * 1e-6));
        let render_every = period as f64 / RATE;
        let quiet = quiet_after(packet, 48_000).as_secs_f64();
        let (mut next_capture, mut next_render) = (capture_every, render_every * 0.37);
        let mut produced = 0_usize;
        let mut block = vec![0.0_f32; packet];
        let silence = vec![0.0_f32; crate::RING_CAPACITY_FRAMES];
        let mut last_packet = 0.0_f64;
        let mut padding = 0_usize;
        let mut primed = false;
        // The first priming is the start of the sound, the rest a ring that ran dry.
        let mut sounding = false;
        let mut out = vec![0_u8; 4 * 8192];
        let mut last: Option<f32> = None;
        let mut run = Run {
            corrections: 0,
            underruns: 0,
            device_starved: 0,
            lowest: usize::MAX,
            highest: 0,
            steepest: 0.0,
            synthesized: 0,
        };
        let settle = 1.0;
        while next_capture.min(next_render) < seconds {
            if next_capture <= next_render {
                let stopped = gap.is_some_and(|(from, to)| (from..to).contains(&next_capture));
                if !stopped {
                    for (offset, sample) in block.iter_mut().enumerate() {
                        *sample = triangle(produced + offset);
                    }
                    produced += packet;
                    ring.push(&block);
                    last_packet = next_capture;
                }
                next_capture += capture_every;
                continue;
            }
            let now = next_render;
            next_render += render_every;
            // The device has played a period since it last woke the lane.
            if primed && padding < period && now > settle {
                run.device_starved += 1;
            }
            padding = padding.saturating_sub(period);
            let want = targets.padding.saturating_sub(padding);
            if now - last_packet >= quiet {
                let lacking = silence_lacking(ring.len(), want, &targets, primed);
                ring.push(&silence[..lacking]);
                run.synthesized += lacking;
            }
            // As the stream thread primes it: the start of a sound keeps what it has, up to the
            // band; a ring that ran dry is set to its target.
            let slack = if sounding { 0 } else { targets.band };
            if !primed && ring.prime(targets.prime(), slack) {
                primed = true;
                sounding = true;
                drift.reset();
            }
            if !primed {
                continue;
            }
            let correction = drift.correction();
            let short = ring.pop_into(&mut out[..want * 4], want, correction);
            padding += want;
            if short > 0 {
                if now > settle {
                    run.underruns += short;
                }
                primed = false;
                last = None;
                continue;
            }
            drift.observe(ring.len());
            if now > settle {
                run.lowest = run.lowest.min(ring.len());
                run.highest = run.highest.max(ring.len());
            }
            for word in out[..want * 4].as_chunks::<4>().0 {
                let sample = f32::from_le_bytes(*word);
                if let Some(previous) = last {
                    run.steepest = run.steepest.max((sample - previous).abs());
                }
                last = Some(sample);
            }
        }
        run.corrections = drift.corrections();
        run
    }

    #[test]
    fn two_clocks_hundreds_of_ppm_apart_play_without_a_gap_or_a_click() {
        for (ppm, packet, period) in [
            (300.0, 480, 480),
            (-300.0, 480, 480),
            (200.0, 441, 480),
            (-200.0, 480, 1056),
        ] {
            let seconds = 120.0;
            let run = simulate(ppm, packet, period, seconds, None);
            let case = format!("{ppm} ppm, packets of {packet}, periods of {period}");
            assert_eq!(run.underruns, 0, "{case}: the ring ran dry");
            assert_eq!(run.device_starved, 0, "{case}: the device ran dry");
            // Once the fill has drifted to the band's edge, the control makes up for exactly the
            // drift; the packets' phase moves where that edge is met by half a block either way.
            let targets = Targets::new(512, packet, period);
            let block = 512.max(packet).max(period) as f64;
            let drift = 48_000.0 * ppm.abs() * 1e-6 * (seconds - 1.0);
            let expected = drift - targets.band as f64;
            let made = run.corrections as f64;
            assert!(
                (made - expected).abs() <= block,
                "{case}: {made} corrections where {expected:.0} were due"
            );
            assert!(
                run.highest <= targets.ring + targets.band + packet.max(period),
                "{case}: the ring grew to {}",
                run.highest
            );
            assert!(run.lowest > 0, "{case}: the ring emptied");
            assert!(
                run.steepest <= TRIANGLE_SLOPE * 1.1,
                "{case}: a step of {} against the signal's {TRIANGLE_SLOPE}",
                run.steepest
            );
        }
    }

    #[test]
    fn two_clocks_in_step_are_left_alone() {
        let run = simulate(0.0, 480, 480, 30.0, None);
        assert_eq!(run.underruns, 0);
        assert_eq!(run.corrections, 0, "nothing to correct for");
        assert!(run.lowest > 0);
        assert_eq!(
            run.synthesized, 0,
            "a source that never stops needs no silence"
        );
        // Packets of another length come in against the device's periods at a phase that moves,
        // which moves the fill read at each write by up to a packet: inside the band.
        let run = simulate(0.0, 441, 480, 30.0, None);
        assert_eq!(run.underruns, 0);
        assert_eq!(run.corrections, 0, "the packets' phase is not drift");
        assert_eq!(run.synthesized, 0);
    }

    #[test]
    fn a_source_that_stops_is_made_up_with_silence_and_one_that_comes_back_plays_on() {
        // The application stops its stream at 10 s, and the loopback delivers nothing until it
        // starts another half a second later.
        for (ppm, packet, period) in [(0.0, 480, 480), (150.0, 441, 480), (-150.0, 480, 1056)] {
            let case = format!("{ppm} ppm, packets of {packet}, periods of {period}");
            let run = simulate(ppm, packet, period, 20.0, Some((10.0, 10.5)));
            assert_eq!(
                run.underruns, 0,
                "{case}: the end of a sound is no underrun"
            );
            assert_eq!(run.device_starved, 0, "{case}: the device ran dry");
            // Half a second of it, give or take the cushion: what the device played meanwhile.
            let gap = 48_000 / 2;
            assert!(
                run.synthesized.abs_diff(gap) <= 2 * 1056,
                "{case}: {} frames of silence for a gap of {gap}",
                run.synthesized
            );
            let targets = Targets::new(512, packet, period);
            assert!(run.lowest > 0, "{case}: the ring emptied");
            assert!(
                run.highest <= targets.ring + targets.band + packet.max(period),
                "{case}: the ring grew to {} once the source came back",
                run.highest
            );
        }
    }

    #[test]
    fn a_quiet_source_is_made_up_to_the_ring_s_target_and_no_further() {
        let targets = Targets::new(512, 480, 480);
        // Primed: enough for the write, and the ring at its target after it.
        assert_eq!(silence_lacking(544, 480, &targets, true), 480 + 1024 - 544);
        assert_eq!(silence_lacking(0, 480, &targets, true), 480 + 1024);
        assert_eq!(
            silence_lacking(4000, 480, &targets, true),
            0,
            "enough already"
        );
        // A sound shorter than the cushion, not yet primed: up to the fill the ring is primed at.
        assert_eq!(
            silence_lacking(300, 480, &targets, false),
            targets.prime() - 300
        );
        assert_eq!(silence_lacking(targets.prime(), 480, &targets, false), 0);
        // Two packets of silence before the source counts as stopped, and never less than 20 ms.
        assert_eq!(quiet_after(480, 48_000), Duration::from_millis(20));
        assert_eq!(quiet_after(960, 48_000), Duration::from_millis(40));
        assert_eq!(quiet_after(128, 48_000), QUIET_FLOOR);
    }
}
