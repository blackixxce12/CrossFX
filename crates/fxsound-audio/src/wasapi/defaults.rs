//! The system's default devices: what FxSound takes, what it remembers and what it gives back.
//!
//! Windows keeps three defaults for each direction, one per `ERole` — `eConsole` (what the sound
//! settings call the default device), `eMultimedia` (the one most applications follow) and
//! `eCommunications` (calls) — and a lane takes all three: the virtual device is what every
//! application plays into, or records from, whichever role it asks for. The original took only
//! `eConsole` (`sndDevicesSet.cpp:60`), which Windows applies to `eMultimedia` with it; the
//! communications default it left where it was, so a call application played past FxSound.
//!
//! Each role is remembered before it is taken. The two playback roles go back to the lane's own
//! device — the one the user picked, or the one the lane played to — as the original's
//! `sndDevicesRestoreDefaultDevice` gives the default back (`sndDevicesSetupDevices.cpp:617-630`);
//! the communications role goes back to what it was, so a user whose calls go to a headset and
//! whose music goes to the speakers gets both back as they were ([`hand_back`]). A role the user
//! has moved off the virtual device since — the desktop's sound settings, a call application — and
//! that FxSound did not take back as a pick is theirs for as long as the claim stands: it is
//! neither claimed again nor written back ([`Claim::yield_role`]).
//!
//! Pure: the writes themselves are `super::com::PolicyConfig`'s.

use std::collections::VecDeque;

/// One of the three roles a default device is kept for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Role {
    Console,
    Multimedia,
    Communications,
}

impl Role {
    /// Every role, in the order they are written: `eConsole` first, as the original wrote it.
    pub(crate) const ALL: [Self; 3] = [Self::Console, Self::Multimedia, Self::Communications];

    const fn index(self) -> usize {
        match self {
            Self::Console => 0,
            Self::Multimedia => 1,
            Self::Communications => 2,
        }
    }

    /// Whether a change of this role's default is the user picking their device. A change of the
    /// communications default is the user picking where calls go, which is not the lane's device —
    /// the original listened to `eMultimedia` alone (`sndDevicesDeviceCallbacks.cpp:95-156`).
    pub(crate) const fn is_pick(self) -> bool {
        match self {
            Self::Console | Self::Multimedia => true,
            Self::Communications => false,
        }
    }
}

/// One direction's default endpoint for each role; `None` where there is none.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Defaults([Option<String>; 3]);

impl Defaults {
    pub(crate) fn get(&self, role: Role) -> Option<&str> {
        self.0[role.index()].as_deref()
    }

    pub(crate) fn set(&mut self, role: Role, id: Option<String>) {
        self.0[role.index()] = id;
    }

    /// The default the device list marks and the settings remember: `eMultimedia`'s, the role the
    /// original read the default from (`sndDevices_GetAll.cpp:126-133`), then `eConsole`'s.
    pub(crate) fn main(&self) -> Option<&str> {
        self.get(Role::Multimedia)
            .or_else(|| self.get(Role::Console))
    }

    /// Whether any role's default is `id`.
    pub(crate) fn any_is(&self, id: &str) -> bool {
        Role::ALL.iter().any(|&role| self.get(role) == Some(id))
    }
}

/// A lane's claim on its direction's defaults: the endpoint it made the default, what each role
/// was before, and the roles the user has taken from it since.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Claim {
    /// The virtual device's endpoint the claim put in every role.
    pub(crate) endpoint: String,
    /// Each role's default from before the claim — never a virtual device's.
    pub(crate) before: Defaults,
    /// The roles the user has moved off the endpoint while the claim stood, and that FxSound did
    /// not take back as a pick: a call application's device picked in the sound settings, or a
    /// playback device picked while a ranking decides the lane's. Theirs until the claim ends.
    yielded: [bool; 3],
}

impl Claim {
    /// A claim on `endpoint` with nothing remembered and nothing yielded: a new one, or one adopted
    /// from a run that left the default on the endpoint.
    pub(crate) fn new(endpoint: String) -> Self {
        Self {
            endpoint,
            before: Defaults::default(),
            yielded: [false; 3],
        }
    }

    /// The user has moved `role` off the endpoint, and FxSound leaves it there: the claim neither
    /// takes it again nor gives it back.
    pub(crate) const fn yield_role(&mut self, role: Role) {
        self.yielded[role.index()] = true;
    }

    /// Whether the user has taken `role` from the claim.
    pub(crate) const fn has_yielded(&self, role: Role) -> bool {
        self.yielded[role.index()]
    }
}

/// The roles whose default is not `endpoint` yet, and that the user has not taken from the claim:
/// what a claim, or a claim taken back after a desktop pick, writes. Each of them is first
/// remembered in `claim`, unless what it holds is a virtual device's (`is_virtual`): the default
/// from before FxSound is never FxSound's.
pub(crate) fn claim_roles(
    current: &Defaults,
    endpoint: &str,
    claim: &mut Claim,
    is_virtual: impl Fn(&str) -> bool,
) -> Vec<Role> {
    let mut writes = Vec::new();
    for role in Role::ALL {
        let now = current.get(role);
        if now == Some(endpoint) || claim.has_yielded(role) {
            continue;
        }
        if let Some(now) = now.filter(|now| !is_virtual(now)) {
            claim.before.set(role, Some(now.to_owned()));
        }
        writes.push(role);
    }
    writes
}

/// The writes that give a claim back. Each role whose default is still the claim's endpoint goes
/// back:
///
/// * a playback role ([`Role::is_pick`]) to `device`, the lane's own device as the device rules
///   remember it — `crate::devices::restore_default_candidate`'s pick, the
///   `sndDevicesRestoreDefaultDevice` order (`sndDevicesSetupDevices.cpp:617-630`): the device
///   the user picked, then the one the lane played to — and failing that to what it was before
///   the claim. The user listened there through FxSound, and hears it there without;
/// * the communications role to what it was before the claim — calls kept on their headset — and
///   failing that to `device`.
///
/// "What it was before" counts only while it is there (`present`). A role with neither is left as
/// it is: writing nothing is better than writing a device that is not there. A role the user has
/// moved off the endpoint since is theirs, and is left alone.
pub(crate) fn hand_back(
    current: &Defaults,
    claim: &Claim,
    device: Option<&str>,
    present: impl Fn(&str) -> bool,
) -> Vec<(Role, String)> {
    Role::ALL
        .into_iter()
        .filter(|&role| current.get(role) == Some(claim.endpoint.as_str()))
        .filter_map(|role| {
            let before = claim
                .before
                .get(role)
                .filter(|&before| before != claim.endpoint.as_str() && present(before));
            let target = if role.is_pick() {
                device.or(before)
            } else {
                before.or(device)
            };
            target.map(|target| (role, target.to_owned()))
        })
        .collect()
}

/// The defaults FxSound has written and has not heard back yet, so that the change notification
/// of each is told apart from the user's own: the Windows twin of the Linux engine's
/// `DefaultState::unconfirmed`.
///
/// Windows reports every change of a default through `IMMNotificationClient::OnDefaultDeviceChanged`,
/// on a thread of its own and in the order it made them — FxSound's writes as much as the sound
/// settings'. A report that matches a write still waiting settles that write and every one before
/// it: an earlier write the system has not reported by now changed nothing (it named what was
/// there already) and never will be.
#[derive(Debug, Default)]
pub(crate) struct Echoes {
    waiting: VecDeque<(Role, String)>,
}

impl Echoes {
    /// How many writes are kept: two claims and two hand-backs' worth, with room to spare. One
    /// that never comes back is dropped for the next rather than held against the user's picks.
    const KEPT: usize = 12;

    /// FxSound has just made `id` the default for `role`.
    pub(crate) fn wrote(&mut self, role: Role, id: &str) {
        if self.waiting.len() == Self::KEPT {
            self.waiting.pop_front();
        }
        self.waiting.push_back((role, id.to_owned()));
    }

    /// Whether `role`'s default becoming `id` is the report of a write of FxSound's.
    pub(crate) fn heard(&mut self, role: Role, id: Option<&str>) -> bool {
        let Some(id) = id else {
            return false;
        };
        let Some(at) = self
            .waiting
            .iter()
            .position(|(written, sent)| *written == role && sent == id)
        else {
            return false;
        };
        self.waiting.drain(..=at);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FX: &str = "fxsound";
    const SPEAKERS: &str = "speakers";
    const HEADSET: &str = "headset";

    fn defaults(console: &str, multimedia: &str, communications: &str) -> Defaults {
        let mut defaults = Defaults::default();
        for (role, id) in Role::ALL
            .into_iter()
            .zip([console, multimedia, communications])
        {
            defaults.set(role, (!id.is_empty()).then(|| id.to_owned()));
        }
        defaults
    }

    fn fresh_claim() -> Claim {
        Claim::new(FX.to_owned())
    }

    #[test]
    fn a_claim_takes_every_role_and_remembers_each_one_s_default() {
        let current = defaults(SPEAKERS, SPEAKERS, HEADSET);
        let mut claim = fresh_claim();
        let writes = claim_roles(&current, FX, &mut claim, |id| id == FX);
        assert_eq!(writes, Role::ALL);
        assert_eq!(
            claim.before, current,
            "calls went to the headset, music to the speakers"
        );
        assert_eq!(claim.before.main(), Some(SPEAKERS));
    }

    #[test]
    fn a_role_already_on_the_virtual_device_is_neither_written_nor_remembered_as_the_user_s() {
        // A killed run left the multimedia role on FxSound: adopted, and nothing remembered for it.
        let current = defaults(SPEAKERS, FX, "");
        let mut claim = fresh_claim();
        assert_eq!(
            claim_roles(&current, FX, &mut claim, |id| id == FX),
            [Role::Console, Role::Communications]
        );
        assert_eq!(claim.before, defaults(SPEAKERS, "", ""));
        // A default on another virtual device is not the user's device either.
        let mut claim = fresh_claim();
        let current = defaults("cable", "cable", "cable");
        claim_roles(&current, FX, &mut claim, |id| id == FX || id == "cable");
        assert_eq!(claim.before, Defaults::default());
    }

    #[test]
    fn the_hand_back_restores_each_role_and_leaves_the_ones_the_user_moved() {
        let mut claim = fresh_claim();
        claim_roles(
            &defaults(SPEAKERS, SPEAKERS, HEADSET),
            FX,
            &mut claim,
            |id| id == FX,
        );
        let present = |_: &str| true;
        assert_eq!(
            hand_back(&defaults(FX, FX, FX), &claim, Some(SPEAKERS), present),
            [
                (Role::Console, SPEAKERS.to_owned()),
                (Role::Multimedia, SPEAKERS.to_owned()),
                (Role::Communications, HEADSET.to_owned()),
            ]
        );
        // The user moved calls to the speakers meanwhile: theirs.
        assert_eq!(
            hand_back(&defaults(FX, FX, SPEAKERS), &claim, Some(SPEAKERS), present),
            [
                (Role::Console, SPEAKERS.to_owned()),
                (Role::Multimedia, SPEAKERS.to_owned()),
            ]
        );
    }

    #[test]
    fn a_role_whose_device_has_gone_goes_to_the_fallback_or_is_left_when_there_is_none() {
        let mut claim = fresh_claim();
        claim_roles(
            &defaults(SPEAKERS, SPEAKERS, HEADSET),
            FX,
            &mut claim,
            |id| id == FX,
        );
        let headset_unplugged = |id: &str| id != HEADSET;
        assert_eq!(
            hand_back(
                &defaults(FX, FX, FX),
                &claim,
                Some(SPEAKERS),
                headset_unplugged
            ),
            [
                (Role::Console, SPEAKERS.to_owned()),
                (Role::Multimedia, SPEAKERS.to_owned()),
                (Role::Communications, SPEAKERS.to_owned()),
            ]
        );
        // Nothing remembered at all, and nothing to fall back to: nothing is written.
        assert!(hand_back(&defaults(FX, FX, FX), &fresh_claim(), None, |_| true).is_empty());
        // A claim adopted from a killed run gives the remembered-nothing roles to the fallback.
        assert_eq!(
            hand_back(
                &defaults(FX, FX, FX),
                &fresh_claim(),
                Some(SPEAKERS),
                |_| true
            )
            .len(),
            3
        );
    }

    #[test]
    fn the_playback_roles_follow_the_lane_s_device_and_calls_go_back_where_they_were() {
        // Music was on the speakers and calls on the headset; the user then picked the headphones
        // in FxSound, and listened there through it.
        let mut claim = fresh_claim();
        claim_roles(
            &defaults(SPEAKERS, SPEAKERS, HEADSET),
            FX,
            &mut claim,
            |id| id == FX,
        );
        assert_eq!(
            hand_back(&defaults(FX, FX, FX), &claim, Some("headphones"), |_| true),
            [
                (Role::Console, "headphones".to_owned()),
                (Role::Multimedia, "headphones".to_owned()),
                (Role::Communications, HEADSET.to_owned()),
            ],
            "where the user listened, and calls where they went"
        );
        // Nothing the rules remember is there: each role goes back to what it was.
        assert_eq!(
            hand_back(&defaults(FX, FX, FX), &claim, None, |_| true),
            [
                (Role::Console, SPEAKERS.to_owned()),
                (Role::Multimedia, SPEAKERS.to_owned()),
                (Role::Communications, HEADSET.to_owned()),
            ]
        );
    }

    #[test]
    fn a_role_the_user_took_from_the_claim_is_neither_claimed_again_nor_given_back() {
        let mut claim = fresh_claim();
        let current = defaults(SPEAKERS, SPEAKERS, HEADSET);
        assert_eq!(
            claim_roles(&current, FX, &mut claim, |id| id == FX).len(),
            3
        );
        // The user puts calls on the speakers while FxSound holds every role.
        claim.yield_role(Role::Communications);
        assert!(claim.has_yielded(Role::Communications));
        assert!(!claim.has_yielded(Role::Console));
        let now = defaults(FX, FX, SPEAKERS);
        assert!(
            claim_roles(&now, FX, &mut claim, |id| id == FX).is_empty(),
            "a later run of the rules takes nothing back"
        );
        assert_eq!(claim.before.get(Role::Communications), Some(HEADSET));
        assert_eq!(
            hand_back(&now, &claim, Some(SPEAKERS), |_| true),
            [
                (Role::Console, SPEAKERS.to_owned()),
                (Role::Multimedia, SPEAKERS.to_owned()),
            ]
        );
        // A new claim starts with every role.
        assert!(!fresh_claim().has_yielded(Role::Communications));
    }

    #[test]
    fn the_report_of_a_write_of_fxsound_s_is_told_apart_from_the_user_s_pick() {
        let mut echoes = Echoes::default();
        echoes.wrote(Role::Console, FX);
        echoes.wrote(Role::Multimedia, FX);
        assert!(echoes.heard(Role::Console, Some(FX)));
        // The user picks the speakers before the second report arrives: not ours.
        assert!(!echoes.heard(Role::Multimedia, Some(SPEAKERS)));
        assert!(echoes.heard(Role::Multimedia, Some(FX)));
        assert!(!echoes.heard(Role::Multimedia, Some(FX)), "settled once");
        assert!(!echoes.heard(Role::Console, None));

        // A report settles every write before it, which the system will never report now.
        echoes.wrote(Role::Console, SPEAKERS);
        echoes.wrote(Role::Console, FX);
        assert!(echoes.heard(Role::Console, Some(FX)));
        assert!(!echoes.heard(Role::Console, Some(SPEAKERS)));

        // And the memory is bounded.
        for _ in 0..100 {
            echoes.wrote(Role::Communications, HEADSET);
        }
        assert_eq!(echoes.waiting.len(), Echoes::KEPT);
    }

    #[test]
    fn only_the_two_playback_roles_are_the_user_picking_a_device() {
        assert!(Role::Console.is_pick());
        assert!(Role::Multimedia.is_pick());
        assert!(!Role::Communications.is_pick());
        let current = defaults(SPEAKERS, "", HEADSET);
        assert_eq!(current.main(), Some(SPEAKERS));
        assert!(current.any_is(HEADSET));
        assert!(!current.any_is(FX));
    }
}
