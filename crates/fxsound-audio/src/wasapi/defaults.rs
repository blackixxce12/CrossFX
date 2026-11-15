//! The system's default devices: what CrossFX takes, what it remembers and what it gives back.
//!
//! Windows keeps three defaults for each direction, one per `ERole` — `eConsole` (what the sound
//! settings call the default device), `eMultimedia` (the one most applications follow) and
//! `eCommunications` (calls). The output lane takes the two playback roles and leaves calls where
//! they are, as the original does: it wrote `eConsole` alone (`sndDevicesSet.cpp:60`), which
//! Windows applies to `eMultimedia` with it. The lane has one real device to play to, so calls
//! taken through it would leave the headset the user chose for them for the lane's speakers for
//! as long as CrossFX runs — and every start would take them again. The input lane takes all
//! three on the cable's recording side: what a call application records is the microphone it is
//! there to process ([`Role::claimed_by`]).
//!
//! Each role is remembered before it is taken. The two playback roles go back to the lane's own
//! device — the one the user picked, or the one the lane played to — as the original's
//! `sndDevicesRestoreDefaultDevice` gives the default back (`sndDevicesSetupDevices.cpp:617-630`);
//! the communications role, where the lane took it, goes back to what it was ([`hand_back`]). A
//! role the user has moved off the virtual device since — the desktop's sound settings, a call
//! application — and that CrossFX did not take back as a pick is theirs for as long as the claim
//! stands: it is neither claimed again nor written back ([`Claim::yield_role`]).
//!
//! Not every default is the user's, though. Windows moves them by itself: a virtual cable just
//! installed is given the first chance at every role of its direction, and a device that goes has
//! its roles fall back to another. A default on a cable that nothing says the user chose is
//! Windows' (`super`, "Whose default it is"), no device the user hears and nothing they chose: it
//! is never remembered as theirs, where it was before is kept ([`Placements`]) for putting it back,
//! and a claim that finds it on its own cable adopts it — gives it back to a real device — rather
//! than leaving it there as theirs.
//!
//! Pure: the writes themselves are `super::com::PolicyConfig`'s.

use std::collections::VecDeque;

use fxsound_core::DeviceDirection;

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

    /// The two playback roles: what the output lane claims.
    pub(crate) const PLAYBACK: [Self; 2] = [Self::Console, Self::Multimedia];

    /// The roles the lane of `direction` makes its virtual endpoint the default for (module
    /// docs): the two playback roles for the output lane, every role for the input lane.
    pub(crate) const fn claimed_by(direction: DeviceDirection) -> &'static [Self] {
        match direction {
            DeviceDirection::Output => &Self::PLAYBACK,
            DeviceDirection::Input => &Self::ALL,
        }
    }

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

    /// Make `role`'s default `id`, and say what it was.
    pub(crate) fn replace(&mut self, role: Role, id: Option<String>) -> Option<String> {
        std::mem::replace(&mut self.0[role.index()], id)
    }

    /// The default the device list marks and the settings remember: `eMultimedia`'s, the role the
    /// original read the default from (`sndDevices_GetAll.cpp:126-133`), then `eConsole`'s.
    pub(crate) fn main(&self) -> Option<&str> {
        self.main_role().and_then(|role| self.get(role))
    }

    /// The role [`Self::main`] reads the default of: `eMultimedia`, or `eConsole` when that has
    /// none; `None` when neither has.
    pub(crate) fn main_role(&self) -> Option<Role> {
        [Role::Multimedia, Role::Console]
            .into_iter()
            .find(|&role| self.get(role).is_some())
    }

    /// Whether any role's default is `id`.
    pub(crate) fn any_is(&self, id: &str) -> bool {
        Role::ALL.iter().any(|&role| self.get(role) == Some(id))
    }
}

/// A role's default moved onto a virtual cable by Windows rather than the user, and where it was
/// before (`super`, "Whose default it is"): the cable just installed, which Windows gives the first
/// chance at every role of its direction; a fallback onto it from a device that went; a move onto a
/// cable one of CrossFX's lanes stands on; one made while the original FxSound ran; or one onto an
/// endpoint that could not be read in time, which is Windows' if it turns out to be a cable. A
/// default found on a cable at the start has none: the engine never saw it anywhere else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Placed {
    /// The endpoint Windows put the role on.
    pub(crate) endpoint: String,
    /// Where the role was before, as Windows last reported it — a chain of Windows' moves from one
    /// cable to the next followed back to where the user had it; `None` for no default at all.
    pub(crate) before: Option<String>,
}

/// One direction's roles whose default Windows moved onto a virtual cable ([`Placed`]), for as long
/// as each stays there: where each goes back to. Not whose a default on a cable is — that is
/// Windows' wherever nothing says the user chose the cable (`super::Engine::windows_default`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Placements([Option<Placed>; 3]);

impl Placements {
    pub(crate) fn get(&self, role: Role) -> Option<&Placed> {
        self.0[role.index()].as_ref()
    }

    pub(crate) fn set(&mut self, role: Role, placed: Option<Placed>) {
        self.0[role.index()] = placed;
    }

    /// Whether Windows put `role`'s default on `id`.
    pub(crate) fn is(&self, role: Role, id: &str) -> bool {
        self.get(role).is_some_and(|placed| placed.endpoint == id)
    }

    /// Forget each role's placement that `current` no longer has the role on: the role has moved
    /// since, and every move is decided anew (`super::Engine::decide_default`). Not for a role
    /// `deciding` says has a move still to be decided, though: that decision reads the placement,
    /// Windows moving a role off a cable it had put it on itself being the same placement.
    pub(crate) fn keep_current(&mut self, current: &Defaults, deciding: impl Fn(Role) -> bool) {
        for role in Role::ALL {
            if !deciding(role) && !current.get(role).is_some_and(|id| self.is(role, id)) {
                self.set(role, None);
            }
        }
    }
}

/// A lane's claim on its direction's defaults: the endpoint it made the default, what each role
/// was before, and the roles the user has taken from it since.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Claim {
    /// The virtual device's endpoint the claim put in the roles its lane takes.
    pub(crate) endpoint: String,
    /// Each role's default from before the claim — never a virtual device's.
    pub(crate) before: Defaults,
    /// The roles the user has moved off the endpoint while the claim stood, and that CrossFX did
    /// not take back as a pick: a call application's device picked in the sound settings, or a
    /// playback device picked while a ranking decides the lane's. Theirs until the claim ends.
    yielded: [bool; 3],
    /// The roles the claim found on its endpoint already, the user's own: a cable they had made
    /// their default, which neither a run of CrossFX's nor Windows by itself put there
    /// ([`Self::finding_theirs`]). Neither adopted nor given back — theirs before CrossFX, and
    /// after it.
    theirs: [bool; 3],
}

impl Claim {
    /// A claim on `endpoint` with nothing remembered and nothing yielded: a new one, or one adopted
    /// from a run that left the default on the endpoint — whatever it finds there is CrossFX's.
    pub(crate) fn new(endpoint: String) -> Self {
        Self {
            endpoint,
            before: Defaults::default(),
            yielded: [false; 3],
            theirs: [false; 3],
        }
    }

    /// A new claim on `endpoint`, a cable's side no run of CrossFX's is known to have left a
    /// default on, `theirs` holding the roles the user put there themselves: every one of them
    /// already on it is the user's own. A user who plays into "CABLE Input" for a streaming
    /// program, or records the desktop from "CABLE Output", made it their default; the lane
    /// standing on that cable takes nothing from them, and gives nothing of theirs away when it
    /// ends. A role Windows put on the cable by itself is not among `theirs` ([`Placements`]): the
    /// claim adopts it, as it adopts one a killed run left, and gives it back to a real device.
    pub(crate) fn finding_theirs(endpoint: String, theirs: &Defaults) -> Self {
        let mut claim = Self::new(endpoint);
        for role in Role::ALL {
            claim.theirs[role.index()] = theirs.get(role) == Some(claim.endpoint.as_str());
        }
        claim
    }

    /// Whether `role` was the user's own on the endpoint when the claim was made.
    pub(crate) const fn is_theirs(&self, role: Role) -> bool {
        self.theirs[role.index()]
    }

    /// Whether the claim holds a role on its endpoint, as `current` stands: one it wrote, or
    /// adopted from a run that left it there — not one of the user's own. A claim holding nothing
    /// is no default a killed run would leave behind.
    pub(crate) fn holds(&self, current: &Defaults) -> bool {
        Role::ALL
            .into_iter()
            .any(|role| current.get(role) == Some(self.endpoint.as_str()) && !self.is_theirs(role))
    }

    /// The user has moved `role` off the endpoint, and CrossFX leaves it there: the claim neither
    /// takes it again nor gives it back.
    pub(crate) const fn yield_role(&mut self, role: Role) {
        self.yielded[role.index()] = true;
    }

    /// Whether the user has taken `role` from the claim.
    pub(crate) const fn has_yielded(&self, role: Role) -> bool {
        self.yielded[role.index()]
    }

    /// The user has picked, for `role`, a device the lane follows: the role is the claim's again,
    /// to be taken back with the move, whatever the user did with it before. A yield is the
    /// user's word on where a role goes while the lane does not follow it; a pick the lane follows
    /// is a later word.
    pub(crate) const fn take_back(&mut self, role: Role) {
        self.yielded[role.index()] = false;
        self.theirs[role.index()] = false;
    }
}

/// The roles among `roles` ([`Role::claimed_by`]) whose default is not `endpoint` yet, and that
/// the user has not taken from the claim: what a claim, or a claim taken back after a desktop pick,
/// writes. Each of them is first remembered in `claim`, as `remembered` says for the default the
/// role has now: that default, for a device of the user's; nothing for a virtual device's — the
/// default from before CrossFX is never CrossFX's — and, for one Windows put on a cable by itself
/// ([`Placements`]), where it was before, if anywhere. A role on `endpoint` already is written
/// nothing, and remembered only where `remembered` has something for it: a role Windows put on the
/// cable the claim stands on, adopted, goes back where it was.
pub(crate) fn claim_roles(
    current: &Defaults,
    endpoint: &str,
    roles: &[Role],
    claim: &mut Claim,
    remembered: impl Fn(Role, &str) -> Option<String>,
) -> Vec<Role> {
    let mut writes = Vec::new();
    for &role in roles {
        if claim.has_yielded(role) {
            continue;
        }
        let now = current.get(role);
        if let Some(before) = now.and_then(|now| remembered(role, now)) {
            claim.before.set(role, Some(before));
        }
        if now == Some(endpoint) {
            continue;
        }
        // Written by CrossFX, so CrossFX's from here on, whoever had it there before.
        claim.theirs[role.index()] = false;
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
///   the claim. The user listened there through CrossFX, and hears it there without;
/// * the communications role to what it was before the claim — calls kept on their headset — and
///   failing that to `device`.
///
/// "What it was before" counts only while it is there (`present`). A role with neither is left as
/// it is: writing nothing is better than writing a device that is not there. A role the user has
/// moved off the endpoint since is theirs, and is left alone; so is one that was theirs on the
/// endpoint before the claim ([`Claim::finding_theirs`]).
pub(crate) fn hand_back(
    current: &Defaults,
    claim: &Claim,
    device: Option<&str>,
    present: impl Fn(&str) -> bool,
) -> Vec<(Role, String)> {
    Role::ALL
        .into_iter()
        .filter(|&role| {
            current.get(role) == Some(claim.endpoint.as_str()) && !claim.is_theirs(role)
        })
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

/// The defaults CrossFX has written and has not heard back yet, so that the change notification
/// of each is told apart from the user's own: the Windows twin of the Linux engine's
/// `DefaultState::unconfirmed`.
///
/// Windows reports every change of a default through `IMMNotificationClient::OnDefaultDeviceChanged`,
/// on a thread of its own and in the order it made them — CrossFX's writes as much as the sound
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

    /// CrossFX has just made `id` the default for `role`.
    pub(crate) fn wrote(&mut self, role: Role, id: &str) {
        if self.waiting.len() == Self::KEPT {
            self.waiting.pop_front();
        }
        self.waiting.push_back((role, id.to_owned()));
    }

    /// Whether `role`'s default becoming `id` is the report of a write of CrossFX's.
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

    /// What a claim remembers of a role's default with CrossFX's the only virtual device and every
    /// other default the user's: that default, unless it is CrossFX's.
    fn users(_: Role, id: &str) -> Option<String> {
        (id != FX).then(|| id.to_owned())
    }

    #[test]
    fn a_claim_takes_every_role_and_remembers_each_one_s_default() {
        let current = defaults(SPEAKERS, SPEAKERS, HEADSET);
        let mut claim = fresh_claim();
        let writes = claim_roles(&current, FX, &Role::ALL, &mut claim, users);
        assert_eq!(writes, Role::ALL);
        assert_eq!(
            claim.before, current,
            "calls went to the headset, music to the speakers"
        );
        assert_eq!(claim.before.main(), Some(SPEAKERS));
    }

    #[test]
    fn the_output_lane_takes_the_playback_roles_and_leaves_calls_where_they_are() {
        assert_eq!(
            Role::claimed_by(DeviceDirection::Output),
            [Role::Console, Role::Multimedia]
        );
        assert_eq!(Role::claimed_by(DeviceDirection::Input), Role::ALL);
        let current = defaults(SPEAKERS, SPEAKERS, HEADSET);
        let mut claim = fresh_claim();
        let writes = claim_roles(
            &current,
            FX,
            Role::claimed_by(DeviceDirection::Output),
            &mut claim,
            users,
        );
        assert_eq!(writes, [Role::Console, Role::Multimedia]);
        assert_eq!(claim.before.get(Role::Communications), None);
        // Calls never left the headset, and nothing gives them back.
        let held = defaults(FX, FX, HEADSET);
        assert_eq!(
            hand_back(&held, &claim, Some(SPEAKERS), |_| true),
            [
                (Role::Console, SPEAKERS.to_owned()),
                (Role::Multimedia, SPEAKERS.to_owned()),
            ]
        );
    }

    #[test]
    fn a_role_already_on_the_virtual_device_is_neither_written_nor_remembered_as_the_user_s() {
        // A killed run left the multimedia role on CrossFX: adopted, and nothing remembered for it.
        let current = defaults(SPEAKERS, FX, "");
        let mut claim = fresh_claim();
        assert_eq!(
            claim_roles(&current, FX, &Role::ALL, &mut claim, users),
            [Role::Console, Role::Communications]
        );
        assert_eq!(claim.before, defaults(SPEAKERS, "", ""));
        // A default on another virtual device is not the user's device either.
        let mut claim = fresh_claim();
        let current = defaults("cable", "cable", "cable");
        claim_roles(&current, FX, &Role::ALL, &mut claim, |_, id| {
            (id != FX && id != "cable").then(|| id.to_owned())
        });
        assert_eq!(claim.before, Defaults::default());
    }

    #[test]
    fn a_role_windows_put_on_the_claim_s_cable_is_adopted_and_goes_back_where_it_was() {
        const CABLE: &str = "cable-out";
        const MICROPHONE: &str = "microphone";
        // Windows gave a cable just installed every recording role, from the microphone; the
        // calls' role it had found there at the start, from nowhere the engine saw.
        let mut placements = Placements::default();
        for role in Role::ALL {
            placements.set(
                role,
                Some(Placed {
                    endpoint: CABLE.to_owned(),
                    before: (role != Role::Communications).then(|| MICROPHONE.to_owned()),
                }),
            );
        }
        let current = defaults(CABLE, CABLE, CABLE);
        // Nothing of it is the user's: the claim adopts every role.
        let theirs = Defaults::default();
        let mut claim = Claim::finding_theirs(CABLE.to_owned(), &theirs);
        let remembered = |role: Role, id: &str| {
            if placements.is(role, id) {
                placements
                    .get(role)
                    .and_then(|placed| placed.before.clone())
            } else {
                (id != CABLE).then(|| id.to_owned())
            }
        };
        assert!(
            claim_roles(&current, CABLE, &Role::ALL, &mut claim, remembered).is_empty(),
            "nothing to write: the roles are on the cable already"
        );
        assert_eq!(claim.before, defaults(MICROPHONE, MICROPHONE, ""));
        assert!(claim.holds(&current), "CrossFX's to give back");
        assert_eq!(
            hand_back(&current, &claim, Some(MICROPHONE), |_| true),
            [
                (Role::Console, MICROPHONE.to_owned()),
                (Role::Multimedia, MICROPHONE.to_owned()),
                (Role::Communications, MICROPHONE.to_owned()),
            ]
        );
        // The same cable made the user's default by the user: theirs, kept and never given away.
        let mut claim = Claim::finding_theirs(CABLE.to_owned(), &current);
        assert!(
            claim_roles(&current, CABLE, &Role::ALL, &mut claim, |_, id| {
                (id != CABLE).then(|| id.to_owned())
            })
            .is_empty()
        );
        assert!(!claim.holds(&current));
        assert!(hand_back(&current, &claim, Some(MICROPHONE), |_| true).is_empty());
    }

    #[test]
    fn a_placement_is_forgotten_once_its_role_has_moved() {
        let mut placements = Placements::default();
        placements.set(
            Role::Multimedia,
            Some(Placed {
                endpoint: "cable".to_owned(),
                before: Some(SPEAKERS.to_owned()),
            }),
        );
        assert!(placements.is(Role::Multimedia, "cable"));
        assert!(!placements.is(Role::Multimedia, SPEAKERS));
        assert!(!placements.is(Role::Console, "cable"));
        placements.keep_current(&defaults(FX, "cable", SPEAKERS), |_| false);
        assert!(placements.is(Role::Multimedia, "cable"), "still there");
        // Moved, with the move still to be decided: the decision reads where Windows had put it.
        let moved = defaults(FX, "other cable", SPEAKERS);
        placements.keep_current(&moved, |role| role == Role::Multimedia);
        assert!(placements.is(Role::Multimedia, "cable"));
        placements.keep_current(&moved, |_| false);
        assert_eq!(placements, Placements::default());
    }

    #[test]
    fn the_hand_back_restores_each_role_and_leaves_the_ones_the_user_moved() {
        let mut claim = fresh_claim();
        claim_roles(
            &defaults(SPEAKERS, SPEAKERS, HEADSET),
            FX,
            &Role::ALL,
            &mut claim,
            users,
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
            &Role::ALL,
            &mut claim,
            users,
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
        // in CrossFX, and listened there through it.
        let mut claim = fresh_claim();
        claim_roles(
            &defaults(SPEAKERS, SPEAKERS, HEADSET),
            FX,
            &Role::ALL,
            &mut claim,
            users,
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
            claim_roles(&current, FX, &Role::ALL, &mut claim, users).len(),
            3
        );
        // The user puts calls on the speakers while CrossFX holds every role.
        claim.yield_role(Role::Communications);
        assert!(claim.has_yielded(Role::Communications));
        assert!(!claim.has_yielded(Role::Console));
        let now = defaults(FX, FX, SPEAKERS);
        assert!(
            claim_roles(&now, FX, &Role::ALL, &mut claim, users).is_empty(),
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
    fn the_report_of_a_write_of_crossfx_s_is_told_apart_from_the_user_s_pick() {
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
        assert_eq!(
            current.main_role(),
            Some(Role::Console),
            "read from eConsole here"
        );
        assert_eq!(
            defaults(FX, SPEAKERS, "").main_role(),
            Some(Role::Multimedia)
        );
        assert_eq!(defaults("", "", HEADSET).main_role(), None);
        assert!(current.any_is(HEADSET));
        assert!(!current.any_is(FX));
    }
}
