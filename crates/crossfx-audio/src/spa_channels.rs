//! The channel positions of `spa/param/audio/raw.h`, as plain numbers.
//!
//! [`crate::devices::ChannelMap`] speaks PipeWire's channel ids, and the output lane's DSP places
//! the subwoofer and the sides by them (`crate::lane_dsp`). Both are shared by the two backends,
//! but only the Linux one links libspa: so the ids live here, as the values of `enum
//! spa_audio_channel`, which is part of SPA's stable ABI, and the Windows backend maps a
//! `WAVEFORMATEXTENSIBLE` channel mask onto the same ids. On Linux a test holds every value to
//! the one `libspa::sys` was built with.

#![allow(dead_code)]

pub const SPA_AUDIO_CHANNEL_UNKNOWN: u32 = 0;
pub const SPA_AUDIO_CHANNEL_NA: u32 = 1;
pub const SPA_AUDIO_CHANNEL_MONO: u32 = 2;
pub const SPA_AUDIO_CHANNEL_FL: u32 = 3;
pub const SPA_AUDIO_CHANNEL_FR: u32 = 4;
pub const SPA_AUDIO_CHANNEL_FC: u32 = 5;
pub const SPA_AUDIO_CHANNEL_LFE: u32 = 6;
pub const SPA_AUDIO_CHANNEL_SL: u32 = 7;
pub const SPA_AUDIO_CHANNEL_SR: u32 = 8;
pub const SPA_AUDIO_CHANNEL_FLC: u32 = 9;
pub const SPA_AUDIO_CHANNEL_FRC: u32 = 10;
pub const SPA_AUDIO_CHANNEL_RC: u32 = 11;
pub const SPA_AUDIO_CHANNEL_RL: u32 = 12;
pub const SPA_AUDIO_CHANNEL_RR: u32 = 13;
pub const SPA_AUDIO_CHANNEL_TC: u32 = 14;
pub const SPA_AUDIO_CHANNEL_TFL: u32 = 15;
pub const SPA_AUDIO_CHANNEL_TFC: u32 = 16;
pub const SPA_AUDIO_CHANNEL_TFR: u32 = 17;
pub const SPA_AUDIO_CHANNEL_TRL: u32 = 18;
pub const SPA_AUDIO_CHANNEL_TRC: u32 = 19;
pub const SPA_AUDIO_CHANNEL_TRR: u32 = 20;
pub const SPA_AUDIO_CHANNEL_RLC: u32 = 21;
pub const SPA_AUDIO_CHANNEL_RRC: u32 = 22;
pub const SPA_AUDIO_CHANNEL_FLW: u32 = 23;
pub const SPA_AUDIO_CHANNEL_FRW: u32 = 24;
pub const SPA_AUDIO_CHANNEL_LFE2: u32 = 25;
pub const SPA_AUDIO_CHANNEL_FLH: u32 = 26;
pub const SPA_AUDIO_CHANNEL_FCH: u32 = 27;
pub const SPA_AUDIO_CHANNEL_FRH: u32 = 28;
pub const SPA_AUDIO_CHANNEL_TFLC: u32 = 29;
pub const SPA_AUDIO_CHANNEL_TFRC: u32 = 30;
pub const SPA_AUDIO_CHANNEL_TSL: u32 = 31;
pub const SPA_AUDIO_CHANNEL_TSR: u32 = 32;
pub const SPA_AUDIO_CHANNEL_LLFE: u32 = 33;
pub const SPA_AUDIO_CHANNEL_RLFE: u32 = 34;
pub const SPA_AUDIO_CHANNEL_BC: u32 = 35;
pub const SPA_AUDIO_CHANNEL_BLC: u32 = 36;
pub const SPA_AUDIO_CHANNEL_BRC: u32 = 37;

/// `SPA_AUDIO_MAX_CHANNELS`: the slots of `spa_audio_info_raw::position`.
pub const SPA_AUDIO_MAX_CHANNELS: usize = 64;

#[cfg(all(test, target_os = "linux"))]
mod tests {
    #[test]
    fn every_id_is_the_one_libspa_was_built_with() {
        use libspa::sys as spa;
        let pairs = [
            (
                super::SPA_AUDIO_CHANNEL_UNKNOWN,
                spa::SPA_AUDIO_CHANNEL_UNKNOWN,
            ),
            (super::SPA_AUDIO_CHANNEL_NA, spa::SPA_AUDIO_CHANNEL_NA),
            (super::SPA_AUDIO_CHANNEL_MONO, spa::SPA_AUDIO_CHANNEL_MONO),
            (super::SPA_AUDIO_CHANNEL_FL, spa::SPA_AUDIO_CHANNEL_FL),
            (super::SPA_AUDIO_CHANNEL_FR, spa::SPA_AUDIO_CHANNEL_FR),
            (super::SPA_AUDIO_CHANNEL_FC, spa::SPA_AUDIO_CHANNEL_FC),
            (super::SPA_AUDIO_CHANNEL_LFE, spa::SPA_AUDIO_CHANNEL_LFE),
            (super::SPA_AUDIO_CHANNEL_SL, spa::SPA_AUDIO_CHANNEL_SL),
            (super::SPA_AUDIO_CHANNEL_SR, spa::SPA_AUDIO_CHANNEL_SR),
            (super::SPA_AUDIO_CHANNEL_FLC, spa::SPA_AUDIO_CHANNEL_FLC),
            (super::SPA_AUDIO_CHANNEL_FRC, spa::SPA_AUDIO_CHANNEL_FRC),
            (super::SPA_AUDIO_CHANNEL_RC, spa::SPA_AUDIO_CHANNEL_RC),
            (super::SPA_AUDIO_CHANNEL_RL, spa::SPA_AUDIO_CHANNEL_RL),
            (super::SPA_AUDIO_CHANNEL_RR, spa::SPA_AUDIO_CHANNEL_RR),
            (super::SPA_AUDIO_CHANNEL_TC, spa::SPA_AUDIO_CHANNEL_TC),
            (super::SPA_AUDIO_CHANNEL_TFL, spa::SPA_AUDIO_CHANNEL_TFL),
            (super::SPA_AUDIO_CHANNEL_TFC, spa::SPA_AUDIO_CHANNEL_TFC),
            (super::SPA_AUDIO_CHANNEL_TFR, spa::SPA_AUDIO_CHANNEL_TFR),
            (super::SPA_AUDIO_CHANNEL_TRL, spa::SPA_AUDIO_CHANNEL_TRL),
            (super::SPA_AUDIO_CHANNEL_TRC, spa::SPA_AUDIO_CHANNEL_TRC),
            (super::SPA_AUDIO_CHANNEL_TRR, spa::SPA_AUDIO_CHANNEL_TRR),
            (super::SPA_AUDIO_CHANNEL_RLC, spa::SPA_AUDIO_CHANNEL_RLC),
            (super::SPA_AUDIO_CHANNEL_RRC, spa::SPA_AUDIO_CHANNEL_RRC),
            (super::SPA_AUDIO_CHANNEL_FLW, spa::SPA_AUDIO_CHANNEL_FLW),
            (super::SPA_AUDIO_CHANNEL_FRW, spa::SPA_AUDIO_CHANNEL_FRW),
            (super::SPA_AUDIO_CHANNEL_LFE2, spa::SPA_AUDIO_CHANNEL_LFE2),
            (super::SPA_AUDIO_CHANNEL_FLH, spa::SPA_AUDIO_CHANNEL_FLH),
            (super::SPA_AUDIO_CHANNEL_FCH, spa::SPA_AUDIO_CHANNEL_FCH),
            (super::SPA_AUDIO_CHANNEL_FRH, spa::SPA_AUDIO_CHANNEL_FRH),
            (super::SPA_AUDIO_CHANNEL_TFLC, spa::SPA_AUDIO_CHANNEL_TFLC),
            (super::SPA_AUDIO_CHANNEL_TFRC, spa::SPA_AUDIO_CHANNEL_TFRC),
            (super::SPA_AUDIO_CHANNEL_TSL, spa::SPA_AUDIO_CHANNEL_TSL),
            (super::SPA_AUDIO_CHANNEL_TSR, spa::SPA_AUDIO_CHANNEL_TSR),
            (super::SPA_AUDIO_CHANNEL_LLFE, spa::SPA_AUDIO_CHANNEL_LLFE),
            (super::SPA_AUDIO_CHANNEL_RLFE, spa::SPA_AUDIO_CHANNEL_RLFE),
            (super::SPA_AUDIO_CHANNEL_BC, spa::SPA_AUDIO_CHANNEL_BC),
            (super::SPA_AUDIO_CHANNEL_BLC, spa::SPA_AUDIO_CHANNEL_BLC),
            (super::SPA_AUDIO_CHANNEL_BRC, spa::SPA_AUDIO_CHANNEL_BRC),
        ];
        for (ours, libspa) in pairs {
            assert_eq!(ours, libspa);
        }
        assert_eq!(
            super::SPA_AUDIO_MAX_CHANNELS,
            libspa::param::audio::MAX_CHANNELS
        );
    }
}
