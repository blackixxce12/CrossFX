//! Stream formats: what an endpoint's mix format says, the format a lane's two streams run at, and
//! the channel layout a `WAVEFORMATEXTENSIBLE` channel mask names.
//!
//! Pure: nothing here calls into Windows. The two Windows structs are only built and read by value
//! (both are `packed(1)` and `Copy`), so all of it is unit-tested and none of it is `unsafe`.
//!
//! # Float, always
//!
//! Every stream a lane opens is 32-bit float, interleaved, which is what the shared-mode engine
//! mixes in and what `crate::lane_dsp` processes. The original relied on the mix format being
//! float without checking (`docs/spec/12-audio-io.md` §8); here the format a lane asks for is
//! always float, and when an endpoint's own mix format is something else the stream is opened with
//! `AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM`, so that the engine converts and the chain never sees
//! integers.

use windows::Win32::Media::Audio::{WAVEFORMATEX, WAVEFORMATEXTENSIBLE, WAVEFORMATEXTENSIBLE_0};
use windows_core::GUID;

use crate::devices::{ChannelMap, MAX_POSITIONS};
use crate::spa_channels::{
    SPA_AUDIO_CHANNEL_FC, SPA_AUDIO_CHANNEL_FL, SPA_AUDIO_CHANNEL_FLC, SPA_AUDIO_CHANNEL_FR,
    SPA_AUDIO_CHANNEL_FRC, SPA_AUDIO_CHANNEL_LFE, SPA_AUDIO_CHANNEL_MONO, SPA_AUDIO_CHANNEL_RC,
    SPA_AUDIO_CHANNEL_RL, SPA_AUDIO_CHANNEL_RR, SPA_AUDIO_CHANNEL_SL, SPA_AUDIO_CHANNEL_SR,
    SPA_AUDIO_CHANNEL_TC, SPA_AUDIO_CHANNEL_TFC, SPA_AUDIO_CHANNEL_TFL, SPA_AUDIO_CHANNEL_TFR,
    SPA_AUDIO_CHANNEL_TRC, SPA_AUDIO_CHANNEL_TRL, SPA_AUDIO_CHANNEL_TRR, SPA_AUDIO_CHANNEL_UNKNOWN,
};
use crate::{DEFAULT_SAMPLE_RATE, MAX_CHANNELS, MAX_SAMPLE_RATE, MIN_CHANNELS};

/// `WAVE_FORMAT_IEEE_FLOAT` (`mmreg.h`). Defined here rather than taken from the `windows` crate,
/// where it lives behind the whole of `Win32_Media_Multimedia` for one number.
pub(crate) const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

/// `WAVE_FORMAT_EXTENSIBLE` (`mmreg.h`), for the same reason.
pub(crate) const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// `KSDATAFORMAT_SUBTYPE_IEEE_FLOAT` (`ksmedia.h`): the sub-format of an extensible float format.
pub(crate) const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: GUID =
    GUID::from_u128(0x0000_0003_0000_0010_8000_00aa_0038_9b71);

/// How many bytes of a `WAVEFORMATEXTENSIBLE` follow its `WAVEFORMATEX`: what its `cbSize` says.
const EXTENSIBLE_EXTRA: u16 =
    (size_of::<WAVEFORMATEXTENSIBLE>() - size_of::<WAVEFORMATEX>()) as u16;

/// The `SPEAKER_*` bits of a channel mask (`ksmedia.h`), in the order the channels they name are
/// interleaved — lowest bit first — with the `spa_audio_channel` id each is to the lanes' DSP.
///
/// `SPEAKER_BACK_*` are SPA's rear channels (`RL`, `RR`, `RC`): Windows' "back" is PipeWire's
/// "rear", and its "side" is PipeWire's "side". So `KSAUDIO_SPEAKER_5POINT1` (`0x3F`) reads as
/// `FL,FR,FC,LFE,RL,RR`, PipeWire's own 5.1, and `KSAUDIO_SPEAKER_7POINT1_SURROUND` (`0x63F`) as
/// `FL,FR,FC,LFE,RL,RR,SL,SR`, its 7.1 ([`ChannelMap::default_for`]).
const SPEAKERS: [(u32, u32); 18] = [
    (0x0000_0001, SPA_AUDIO_CHANNEL_FL),
    (0x0000_0002, SPA_AUDIO_CHANNEL_FR),
    (0x0000_0004, SPA_AUDIO_CHANNEL_FC),
    (0x0000_0008, SPA_AUDIO_CHANNEL_LFE),
    (0x0000_0010, SPA_AUDIO_CHANNEL_RL),
    (0x0000_0020, SPA_AUDIO_CHANNEL_RR),
    (0x0000_0040, SPA_AUDIO_CHANNEL_FLC),
    (0x0000_0080, SPA_AUDIO_CHANNEL_FRC),
    (0x0000_0100, SPA_AUDIO_CHANNEL_RC),
    (0x0000_0200, SPA_AUDIO_CHANNEL_SL),
    (0x0000_0400, SPA_AUDIO_CHANNEL_SR),
    (0x0000_0800, SPA_AUDIO_CHANNEL_TC),
    (0x0000_1000, SPA_AUDIO_CHANNEL_TFL),
    (0x0000_2000, SPA_AUDIO_CHANNEL_TFC),
    (0x0000_4000, SPA_AUDIO_CHANNEL_TFR),
    (0x0000_8000, SPA_AUDIO_CHANNEL_TRL),
    (0x0001_0000, SPA_AUDIO_CHANNEL_TRC),
    (0x0002_0000, SPA_AUDIO_CHANNEL_TRR),
];

/// What an endpoint's mix format says (`IAudioClient::GetMixFormat`): the format the shared-mode
/// engine mixes the endpoint at, read out of its `WAVEFORMATEX` or `WAVEFORMATEXTENSIBLE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MixFormat {
    /// `wFormatTag`.
    pub(crate) tag: u16,
    pub(crate) channels: u16,
    pub(crate) rate: u32,
    /// `wBitsPerSample`: the container size.
    pub(crate) bits: u16,
    /// `dwChannelMask`, for an extensible format; `None` for a plain `WAVEFORMATEX`, whose
    /// channels Windows lays out by their count.
    pub(crate) mask: Option<u32>,
    /// `SubFormat`, for an extensible format.
    pub(crate) sub_format: Option<GUID>,
}

impl MixFormat {
    /// Read a plain `WAVEFORMATEX`, or the head of an extensible one whose tail was not there to
    /// read.
    pub(crate) fn from_ex(format: &WAVEFORMATEX) -> Self {
        // Fields of a `packed(1)` struct are copied out by value: a reference to one would be
        // unaligned.
        let WAVEFORMATEX {
            wFormatTag,
            nChannels,
            nSamplesPerSec,
            wBitsPerSample,
            ..
        } = *format;
        Self {
            tag: wFormatTag,
            channels: nChannels,
            rate: nSamplesPerSec,
            bits: wBitsPerSample,
            mask: None,
            sub_format: None,
        }
    }

    /// Read a `WAVEFORMATEXTENSIBLE`, the form nearly every shared-mode mix format takes.
    pub(crate) fn from_extensible(format: &WAVEFORMATEXTENSIBLE) -> Self {
        let WAVEFORMATEXTENSIBLE {
            Format,
            dwChannelMask,
            SubFormat,
            ..
        } = *format;
        Self {
            mask: Some(dwChannelMask),
            sub_format: Some(SubFormat),
            ..Self::from_ex(&Format)
        }
    }

    /// Whether the format is 32-bit float, plain or extensible: what a lane's streams run at, and
    /// what nearly every shared-mode endpoint mixes at.
    pub(crate) fn is_float32(&self) -> bool {
        self.bits == 32
            && match self.tag {
                WAVE_FORMAT_IEEE_FLOAT => true,
                WAVE_FORMAT_EXTENSIBLE => self.sub_format == Some(KSDATAFORMAT_SUBTYPE_IEEE_FLOAT),
                _ => false,
            }
    }

    /// The channel layout the format names: its mask's speakers, or Windows' layout for the count
    /// when it has none.
    pub(crate) fn layout(&self) -> ChannelMap {
        channel_map(self.mask.unwrap_or(0), usize::from(self.channels))
    }
}

/// The format both streams of a lane run at: 32-bit float, interleaved, `channels` channels at
/// `rate`, laid out as `mask` says. The capture stream is opened at it, and the render stream is
/// told it and left to convert (`AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM`), so that the ring between the
/// two never has to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StreamFormat {
    pub(crate) rate: u32,
    pub(crate) channels: u16,
    pub(crate) mask: u32,
}

impl StreamFormat {
    /// The format the output lane captures the virtual device at: the device's own rate, and its
    /// own channels clamped to the `2..=8` the chain runs (`sndDevices.h:190-191`).
    ///
    /// The device's own rate, unlike the original, which pushed 44.1 or 48 kHz onto its driver
    /// through `IPolicyConfig::SetDeviceFormat` (`docs/spec/12-audio-io.md` §8): the virtual
    /// device's format is the user's setting, and the render stream's converter takes the lane to
    /// whatever the real device runs at. `None` for a rate no chain runs at.
    pub(crate) fn for_loopback(mix: &MixFormat) -> Option<Self> {
        if mix.rate == 0 || mix.rate > MAX_SAMPLE_RATE {
            return None;
        }
        let channels = clamped_channels(mix.channels);
        Some(Self {
            rate: mix.rate,
            channels,
            mask: if channels == mix.channels {
                mix.mask
                    .filter(|mask| mask.count_ones() >= u32::from(channels))
                    .unwrap_or_else(|| default_mask(channels))
            } else {
                default_mask(channels)
            },
        })
    }

    /// The format the input lane records the microphone at: 48 kHz whatever the microphone runs
    /// at, and at least a stereo pair — what the Linux capture stream asks PipeWire for
    /// (`crate::engine`, `CAPTURE_RATE`), so that the voice chain runs the same on both. The
    /// engine's converter up-mixes a mono microphone into the pair and resamples a 16 kHz headset;
    /// what the microphone really carries reaches the de-esser as the source rate.
    pub(crate) fn for_microphone(mix: &MixFormat) -> Self {
        let channels = clamped_channels(mix.channels);
        Self {
            rate: DEFAULT_SAMPLE_RATE,
            channels,
            mask: if channels == mix.channels {
                mix.mask
                    .filter(|mask| mask.count_ones() >= u32::from(channels))
                    .unwrap_or_else(|| default_mask(channels))
            } else {
                default_mask(channels)
            },
        }
    }

    /// Bytes per frame: four per channel.
    pub(crate) const fn block_align(&self) -> usize {
        self.channels as usize * size_of::<f32>()
    }

    /// Whether `mix` already is this format, so that a stream can be opened at the mix format
    /// itself rather than asking the engine to convert.
    pub(crate) fn is(&self, mix: &MixFormat) -> bool {
        mix.is_float32()
            && mix.rate == self.rate
            && mix.channels == self.channels
            && mix.mask.is_none_or(|mask| mask == self.mask)
    }

    /// The channel layout the lane's DSP is told ([`crate::lane_dsp::LaneDsp::set_layout`]).
    pub(crate) fn layout(&self) -> ChannelMap {
        channel_map(self.mask, usize::from(self.channels))
    }

    /// The `WAVEFORMATEXTENSIBLE` a stream is opened with.
    pub(crate) fn to_wave_format(self) -> WAVEFORMATEXTENSIBLE {
        let block_align = self.channels * size_of::<f32>() as u16;
        WAVEFORMATEXTENSIBLE {
            Format: WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_EXTENSIBLE,
                nChannels: self.channels,
                nSamplesPerSec: self.rate,
                nAvgBytesPerSec: self.rate * u32::from(block_align),
                nBlockAlign: block_align,
                wBitsPerSample: 32,
                cbSize: EXTENSIBLE_EXTRA,
            },
            Samples: WAVEFORMATEXTENSIBLE_0 {
                wValidBitsPerSample: 32,
            },
            dwChannelMask: self.mask,
            SubFormat: KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
        }
    }

    /// Frames of this format in `hns` 100-nanosecond units (`REFERENCE_TIME`), rounded up.
    pub(crate) fn frames_in(&self, hns: i64) -> usize {
        let hns = u64::try_from(hns).unwrap_or(0);
        usize::try_from((hns * u64::from(self.rate)).div_ceil(10_000_000)).unwrap_or(usize::MAX)
    }
}

/// A channel count clamped to what the chain runs: `2..=8`.
fn clamped_channels(channels: u16) -> u16 {
    channels.clamp(MIN_CHANNELS as u16, MAX_CHANNELS as u16)
}

/// The channel layout `mask` names for `channels` channels, as `spa_audio_channel` ids
/// ([`crate::spa_channels`]) — the layout the output lane's DSP places the subwoofer, the front
/// pair and the sides by.
///
/// The mask's speakers in the order of their bits, which is the order of the channels
/// (`WAVEFORMATEXTENSIBLE`). A mask naming more speakers than there are channels names the first
/// ones, as Windows reads it; one naming fewer leaves the remaining channels tied to no speaker,
/// which the chain treats as centre. A mask of zero names nothing, and the layout is the one
/// PipeWire's converter — and Windows' — uses for the count ([`ChannelMap::default_for`]).
pub(crate) fn channel_map(mask: u32, channels: usize) -> ChannelMap {
    let channels = channels.clamp(1, MAX_POSITIONS);
    if mask == 0 {
        return ChannelMap::default_for(channels as u32);
    }
    let mut ids = [SPA_AUDIO_CHANNEL_UNKNOWN; MAX_POSITIONS];
    for (slot, &(_, id)) in ids
        .iter_mut()
        .zip(SPEAKERS.iter().filter(|&&(bit, _)| mask & bit != 0))
    {
        *slot = id;
    }
    // One channel named by its centre speaker is Windows' mono (`KSAUDIO_SPEAKER_MONO`).
    if channels == 1 && ids[0] == SPA_AUDIO_CHANNEL_FC {
        ids[0] = SPA_AUDIO_CHANNEL_MONO;
    }
    ChannelMap::from_ids(&ids[..channels]).unwrap_or_else(|| ChannelMap::default_for(2))
}

/// The channel mask of the layout PipeWire — and Windows — uses for `channels` channels
/// ([`ChannelMap::default_for`]): what a stream whose channel count was clamped is opened with.
pub(crate) fn default_mask(channels: u16) -> u32 {
    let speaker = |id: u32| match id {
        // Mono is the centre speaker's (`KSAUDIO_SPEAKER_MONO`).
        SPA_AUDIO_CHANNEL_MONO => 0x4,
        id => SPEAKERS
            .iter()
            .find(|&&(_, speaker)| speaker == id)
            .map_or(0, |&(bit, _)| bit),
    };
    ChannelMap::default_for(u32::from(channels))
        .ids()
        .iter()
        .fold(0, |mask, &id| mask | speaker(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn float_mix(channels: u16, rate: u32, mask: Option<u32>) -> MixFormat {
        MixFormat {
            tag: WAVE_FORMAT_EXTENSIBLE,
            channels,
            rate,
            bits: 32,
            mask,
            sub_format: Some(KSDATAFORMAT_SUBTYPE_IEEE_FLOAT),
        }
    }

    #[test]
    fn the_masks_windows_ships_read_as_pipewire_s_own_layouts() {
        // KSAUDIO_SPEAKER_STEREO, _5POINT1, _5POINT1_SURROUND, _7POINT1_SURROUND.
        assert_eq!(channel_map(0x3, 2), ChannelMap::default_for(2));
        assert_eq!(channel_map(0x3F, 6), ChannelMap::default_for(6));
        assert_eq!(channel_map(0x63F, 8), ChannelMap::default_for(8));
        assert_eq!(
            channel_map(0x60F, 6),
            ChannelMap::parse("FL,FR,FC,LFE,SL,SR").expect("a layout"),
            "5.1 with side speakers, the mask the original's driver takes (`sndDevicesSet.cpp:504`)"
        );
        // KSAUDIO_SPEAKER_QUAD: the backs are PipeWire's rears.
        assert_eq!(
            channel_map(0x33, 4),
            ChannelMap::parse("FL,FR,RL,RR").expect("a layout")
        );
        assert_eq!(
            channel_map(0x4, 1),
            ChannelMap::default_for(1),
            "mono is the centre"
        );
    }

    #[test]
    fn a_layout_from_a_mask_places_the_subwoofer_and_the_front_pair_where_the_mask_puts_them() {
        // 5.1 from the original's driver mask: the LFE is the fourth channel, whatever it is called.
        let map = channel_map(0x60F, 6);
        assert_eq!(map.lfe_index(), Some(3));
        assert_eq!(map.front_pair(), Some((0, 1)));
        // A mask with the front pair alone, on a device that orders it after the centre: there is
        // no such Windows ordering, the bits fix it — FL is always channel 0.
        let map = channel_map(0x7, 3);
        assert_eq!(map.front_pair(), Some((0, 1)));
        assert_eq!(map.lfe_index(), None);
    }

    #[test]
    fn a_mask_that_disagrees_with_the_channel_count_is_read_as_windows_reads_it() {
        // More speakers than channels: the first ones.
        assert_eq!(channel_map(0x3F, 2), ChannelMap::default_for(2));
        // Fewer: the rest are tied to no speaker.
        let map = channel_map(0x3, 4);
        assert_eq!(map.len(), 4);
        assert_eq!(map.front_pair(), Some((0, 1)));
        assert_eq!(
            &map.ids()[2..],
            &[SPA_AUDIO_CHANNEL_UNKNOWN, SPA_AUDIO_CHANNEL_UNKNOWN]
        );
        // None at all: the count decides.
        assert_eq!(channel_map(0, 6), ChannelMap::default_for(6));
        // More channels than the chain runs are not a layout to place anything by.
        assert_eq!(channel_map(0xFFFF, 12).len(), MAX_POSITIONS);
    }

    #[test]
    fn a_layout_is_made_of_one_to_eight_ids_in_their_order_and_of_nothing_else() {
        let ids = [
            SPA_AUDIO_CHANNEL_FL,
            SPA_AUDIO_CHANNEL_FR,
            SPA_AUDIO_CHANNEL_LFE,
        ];
        let map = ChannelMap::from_ids(&ids).expect("three ids");
        assert_eq!(map.ids(), ids);
        assert_eq!(map.lfe_index(), Some(2));
        assert_eq!(ChannelMap::from_ids(&[]), None, "no channels is no layout");
        assert_eq!(
            ChannelMap::from_ids(&[SPA_AUDIO_CHANNEL_FL; MAX_POSITIONS + 1]),
            None,
            "more than the chain runs"
        );
        assert!(ChannelMap::from_ids(&[SPA_AUDIO_CHANNEL_MONO; MAX_POSITIONS]).is_some());
    }

    #[test]
    fn the_default_masks_are_the_ones_windows_names_for_each_count() {
        assert_eq!(default_mask(1), 0x4, "KSAUDIO_SPEAKER_MONO");
        assert_eq!(default_mask(2), 0x3, "KSAUDIO_SPEAKER_STEREO");
        assert_eq!(default_mask(4), 0x33, "KSAUDIO_SPEAKER_QUAD");
        assert_eq!(default_mask(6), 0x3F, "KSAUDIO_SPEAKER_5POINT1");
        assert_eq!(default_mask(8), 0x63F, "KSAUDIO_SPEAKER_7POINT1_SURROUND");
        // And every one reads back as the layout it came from.
        for channels in 1..=8_u16 {
            assert_eq!(
                channel_map(default_mask(channels), usize::from(channels)),
                ChannelMap::default_for(u32::from(channels)),
                "{channels} channels"
            );
        }
    }

    #[test]
    fn the_loopback_runs_at_the_virtual_device_s_own_rate_and_a_clamped_channel_count() {
        let stereo = float_mix(2, 48_000, Some(0x3));
        let format = StreamFormat::for_loopback(&stereo).expect("a format");
        assert_eq!(
            (format.rate, format.channels, format.mask),
            (48_000, 2, 0x3)
        );
        assert!(format.is(&stereo), "opened at the mix format itself");

        // A mono virtual device runs the chain's stereo pair, converted by the engine.
        let mono = float_mix(1, 44_100, Some(0x4));
        let format = StreamFormat::for_loopback(&mono).expect("a format");
        assert_eq!(
            (format.rate, format.channels, format.mask),
            (44_100, 2, 0x3)
        );
        assert!(!format.is(&mono));

        // Twelve channels are eight.
        let many = float_mix(12, 48_000, Some(0x3FFFF));
        let format = StreamFormat::for_loopback(&many).expect("a format");
        assert_eq!((format.channels, format.mask), (8, 0x63F));

        // A 16-bit mix format is converted, not taken as float.
        let pcm = MixFormat {
            tag: 1,
            bits: 16,
            sub_format: None,
            mask: None,
            ..stereo
        };
        assert!(!pcm.is_float32());
        assert!(!StreamFormat::for_loopback(&pcm).expect("a format").is(&pcm));

        // No rate, or one past what the chain runs at, is no format.
        assert_eq!(StreamFormat::for_loopback(&float_mix(2, 0, None)), None);
        assert_eq!(
            StreamFormat::for_loopback(&float_mix(2, 384_000, None)),
            None
        );
    }

    #[test]
    fn the_microphone_is_recorded_at_48_khz_and_at_least_a_stereo_pair() {
        let headset = float_mix(1, 16_000, Some(0x4));
        let format = StreamFormat::for_microphone(&headset);
        assert_eq!(
            (format.rate, format.channels, format.mask),
            (48_000, 2, 0x3)
        );
        let array = float_mix(4, 48_000, Some(0x33));
        let format = StreamFormat::for_microphone(&array);
        assert_eq!((format.channels, format.mask), (4, 0x33));
        assert!(format.is(&array));
    }

    #[test]
    fn the_wave_format_a_stream_is_opened_with_says_what_the_lane_runs() {
        let format = StreamFormat {
            rate: 96_000,
            channels: 6,
            mask: 0x3F,
        };
        let wave = format.to_wave_format();
        let read = MixFormat::from_extensible(&wave);
        assert!(read.is_float32());
        assert_eq!(read.channels, 6);
        assert_eq!(read.rate, 96_000);
        assert_eq!(read.mask, Some(0x3F));
        let WAVEFORMATEX {
            nBlockAlign,
            nAvgBytesPerSec,
            cbSize,
            ..
        } = wave.Format;
        assert_eq!(nBlockAlign, 24);
        assert_eq!(nAvgBytesPerSec, 96_000 * 24);
        assert_eq!(cbSize, 22, "the extensible tail, as Windows sizes it");
        assert_eq!(format.block_align(), 24);
        assert!(format.is(&read));
    }

    #[test]
    fn reference_times_convert_to_frames_rounding_up() {
        let format = StreamFormat {
            rate: 48_000,
            channels: 2,
            mask: 0x3,
        };
        // The default shared-mode period, 10 ms.
        assert_eq!(format.frames_in(100_000), 480);
        assert_eq!(format.frames_in(1), 1);
        assert_eq!(format.frames_in(0), 0);
        assert_eq!(format.frames_in(-5), 0);
        let cd = StreamFormat {
            rate: 44_100,
            ..format
        };
        assert_eq!(cd.frames_in(100_000), 441);
    }
}
