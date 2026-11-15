//! CrossFX's own record of the defaults it holds: the endpoint each lane has made its direction's
//! default, written before the first write of a claim and cleared once its hand-back has moved
//! every role off the endpoint — and of the defaults the user put on a virtual cable themselves.
//!
//! What the record is for is the run that never hands back. The release profile aborts on a panic
//! and a process can be killed, and either leaves the defaults on the virtual device, which
//! swallows every sound until something gives them back. The next start does (`super`, "The
//! default devices") — but only a default it can tell is CrossFX's. A virtual cable is not
//! CrossFX's alone: a user who records the desktop's sound may have made "CABLE Output" their
//! default recording device, or play into "CABLE Input" on purpose, and a start that moved those
//! defaults away would break their setup at every start — the input lane, detached until a
//! microphone is picked, never stands behind its cable. So a default left on a cable is given back
//! only when the record says a run of CrossFX put it there. A default on the original FxSound's
//! driver needs no record: nothing plays that device while neither the original nor CrossFX runs.
//! (A lane that comes to stand on a cable later decides for itself whose each default it finds
//! there is — the user's, or one Windows put there by itself: `super`, "Whose default it is".)
//!
//! The other way round, a default the user put on a cable is theirs ([`Kept::users`]). A start
//! finds it there with nothing to tell it from one Windows put there as it installed the cable —
//! and Windows' it takes it for, which CrossFX moves wherever it is in that default's path. What
//! says the user chose it is that a run of CrossFX saw them move it there; that is kept here, role
//! by role, for as long as the role stays there, so that a later start leaves a call application
//! sent to "CABLE Input", or a recorder of the desktop on "CABLE Output", where the user put it.
//! It is forgotten once the role is somewhere else: moved by anyone but CrossFX while a run
//! watches, or found elsewhere by the next start — anywhere but on the endpoint CrossFX holds,
//! where a run killed while it held the role left it.
//!
//! The Linux engine needs none of this: its virtual nodes are its own, and a default naming one is
//! CrossFX's by its name.
//!
//! The file is `%LOCALAPPDATA%\CrossFX\claimed-defaults.toml`, in CrossFX's own local folder
//! (`fxsound_core::Settings::local_dir`) — local, not roaming, because an endpoint ID names a device
//! of one machine. Never the original FxSound's own local folder, which Windows does not tell
//! from one called `fxsound`:
//!
//! ```toml
//! output = "{0.0.0.00000000}.{73ca9fe5-1336-4bd0-b0a4-149e64acf557}"
//!
//! [users.output]
//! communications = "{0.0.0.00000000}.{0b5f1e8e-7a3c-4d0b-9c8e-2f1a4d6b3c21}"
//! ```
//!
//! Written whole, to a temporary file renamed over it, so that a run killed in the middle of a
//! write leaves the old record or the new one, never half of one; removed once it holds nothing.
//! Read once, at the start. A record that cannot be read is no record, and a record that cannot be
//! written is logged: neither is worth more than the sound.

use std::path::PathBuf;

use fxsound_core::Settings;
use serde::{Deserialize, Serialize};

use super::defaults::{Defaults, Role};
use crate::per_direction::PerDirection;

/// The file's name, in [`Record::of_user`]'s directory.
const FILE_NAME: &str = "claimed-defaults.toml";

/// What the record keeps.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Kept {
    /// The endpoint each lane holds its direction's defaults on, or nothing.
    pub(crate) claimed: PerDirection<Option<String>>,
    /// Per direction and role, the virtual cable the user made that role's default themselves
    /// while a run of CrossFX watched (`super`, "Whose default it is"): theirs at a later start
    /// too, which would otherwise find it there with nothing to tell it from one Windows put there.
    pub(crate) users: PerDirection<Defaults>,
}

/// The record's contents: each lane's endpoint, or nothing, and the user's own cable defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Contents {
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<String>,
    #[serde(skip_serializing_if = "Users::is_empty")]
    users: Users,
}

/// The user's own cable defaults, per direction.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Users {
    #[serde(skip_serializing_if = "Roles::is_empty")]
    output: Roles,
    #[serde(skip_serializing_if = "Roles::is_empty")]
    input: Roles,
}

impl Users {
    fn is_empty(&self) -> bool {
        self.output.is_empty() && self.input.is_empty()
    }
}

/// One direction's endpoint per role, or nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Roles {
    #[serde(skip_serializing_if = "Option::is_none")]
    console: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    multimedia: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    communications: Option<String>,
}

impl Roles {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    fn slot(&mut self, role: Role) -> &mut Option<String> {
        match role {
            Role::Console => &mut self.console,
            Role::Multimedia => &mut self.multimedia,
            Role::Communications => &mut self.communications,
        }
    }
}

impl From<&Defaults> for Roles {
    fn from(defaults: &Defaults) -> Self {
        let mut roles = Self::default();
        for role in Role::ALL {
            *roles.slot(role) = defaults.get(role).map(str::to_owned);
        }
        roles
    }
}

impl From<Roles> for Defaults {
    fn from(mut roles: Roles) -> Self {
        let mut defaults = Self::default();
        for role in Role::ALL {
            defaults.set(
                role,
                roles.slot(role).take().filter(|id| !id.trim().is_empty()),
            );
        }
        defaults
    }
}

impl From<&Kept> for Contents {
    fn from(kept: &Kept) -> Self {
        Self {
            output: kept.claimed.output.clone(),
            input: kept.claimed.input.clone(),
            users: Users {
                output: Roles::from(&kept.users.output),
                input: Roles::from(&kept.users.input),
            },
        }
    }
}

/// Where the record is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Record {
    /// `None` when there is nowhere to keep it — Windows names no local application data — and
    /// then nothing is kept.
    path: Option<PathBuf>,
}

impl Record {
    /// The user's record, in CrossFX's local folder (`%LOCALAPPDATA%\CrossFX`,
    /// [`Settings::local_dir`]): the folder the shell names, not the environment.
    pub(crate) fn of_user() -> Self {
        Self {
            path: Settings::local_dir().map(|dir| dir.join(FILE_NAME)),
        }
    }

    /// A record kept at `path`: a test's own file.
    #[cfg(test)]
    pub(crate) const fn at(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    /// What the record holds; nothing for a record that is not there or cannot be read.
    pub(crate) fn read(&self) -> Kept {
        let Some(path) = self.path.as_ref() else {
            return Kept::default();
        };
        match std::fs::read_to_string(path) {
            Ok(text) => parse(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Kept::default(),
            Err(error) => {
                log::warn!("{}: {error}; taken as no record", path.display());
                Kept::default()
            }
        }
    }

    /// Keep `kept`, replacing whatever the record held; remove the file when it holds nothing.
    pub(crate) fn write(&self, kept: &Kept) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        let written = match render(kept) {
            None => match std::fs::remove_file(path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                other => other,
            },
            Some(text) => {
                let temporary = path.with_extension("toml.tmp");
                path.parent()
                    .map_or(Ok(()), std::fs::create_dir_all)
                    .and_then(|()| std::fs::write(&temporary, text))
                    .and_then(|()| std::fs::rename(&temporary, path))
            }
        };
        if let Err(error) = written {
            log::warn!(
                "{}: {error}; a run killed now leaves its default for the user to give back",
                path.display()
            );
        }
    }
}

/// The record a file's `text` holds. Anything unreadable in it is no record of that lane.
fn parse(text: &str) -> Kept {
    let contents: Contents = toml::from_str(text).unwrap_or_else(|error| {
        log::warn!("the record of CrossFX's defaults is unreadable ({error}); taken as no record");
        Contents::default()
    });
    let keep = |id: Option<String>| id.filter(|id| !id.trim().is_empty());
    Kept {
        claimed: PerDirection {
            output: keep(contents.output),
            input: keep(contents.input),
        },
        users: PerDirection {
            output: contents.users.output.into(),
            input: contents.users.input.into(),
        },
    }
}

/// The file `kept` is kept as, or `None` when it holds nothing and the file goes.
fn render(kept: &Kept) -> Option<String> {
    let contents = Contents::from(kept);
    if contents == Contents::default() {
        return None;
    }
    let body = toml::to_string(&contents)
        .map_err(|error| log::warn!("the record of CrossFX's defaults: {error}"))
        .ok()?;
    Some(format!(
        "# The default devices CrossFX holds, so that a start after a run that was killed gives \
         them back; and the ones\n# the user put on a virtual cable themselves, so that a later \
         start leaves them theirs.\n# Written and removed by CrossFX itself.\n{body}"
    ))
}

#[cfg(test)]
mod tests {
    use fxsound_core::DeviceDirection;

    use super::*;

    const FX: &str = "{0.0.0.00000000}.{73ca9fe5-1336-4bd0-b0a4-149e64acf557}";
    const CABLE_IN: &str = "{0.0.0.00000000}.{cable-in}";
    const CABLE_OUT: &str = "{0.0.1.00000000}.{cable-out}";

    fn claimed(output: Option<&str>, input: Option<&str>) -> Kept {
        Kept {
            claimed: PerDirection {
                output: output.map(str::to_owned),
                input: input.map(str::to_owned),
            },
            users: PerDirection::default(),
        }
    }

    /// `kept` with the user's own cable default for `role` of `direction`.
    fn theirs(mut kept: Kept, direction: DeviceDirection, role: Role, id: &str) -> Kept {
        kept.users.get_mut(direction).set(role, Some(id.to_owned()));
        kept
    }

    #[test]
    fn a_record_reads_back_as_it_was_written_lane_by_lane() {
        for record in [
            claimed(Some(FX), None),
            claimed(None, Some(CABLE_OUT)),
            claimed(Some(FX), Some(CABLE_OUT)),
            theirs(
                claimed(Some(FX), None),
                DeviceDirection::Output,
                Role::Communications,
                CABLE_IN,
            ),
            theirs(
                theirs(
                    claimed(None, None),
                    DeviceDirection::Input,
                    Role::Console,
                    CABLE_OUT,
                ),
                DeviceDirection::Input,
                Role::Multimedia,
                CABLE_OUT,
            ),
        ] {
            let text = render(&record).expect("something to keep");
            assert!(text.starts_with('#'), "says what it is: {text}");
            assert_eq!(parse(&text), record, "{text}");
        }
        assert_eq!(render(&claimed(None, None)), None, "nothing held, no file");
    }

    #[test]
    fn a_record_that_is_damaged_or_empty_is_no_record() {
        assert_eq!(parse(""), claimed(None, None));
        assert_eq!(parse("output = \n"), claimed(None, None));
        assert_eq!(parse("output = 3"), claimed(None, None));
        assert_eq!(parse("output = \"  \""), claimed(None, None));
        assert_eq!(
            parse("[users.output]\ncommunications = \" \"\n"),
            claimed(None, None)
        );
        // Another key beside it is not the record's business.
        assert_eq!(
            parse("output = \"x\"\nlater = true\n[users.input]\nconsole = \"y\"\nlater = 1\n"),
            theirs(
                claimed(Some("x"), None),
                DeviceDirection::Input,
                Role::Console,
                "y"
            )
        );
        // A record from before the user's own cables were kept reads as it did.
        assert_eq!(
            parse(&format!("output = \"{FX}\"\n")),
            claimed(Some(FX), None)
        );
    }

    #[test]
    fn the_record_is_kept_in_its_file_and_removed_when_it_holds_nothing() {
        let dir = fxsound_core::test_support::ScratchDir::new("wasapi-record");
        let path = dir.path().join("crossfx").join(FILE_NAME);
        let record = Record::at(path.clone());
        assert_eq!(record.read(), claimed(None, None), "no file yet");
        record.write(&claimed(Some(FX), None));
        assert!(path.is_file(), "the directory is made for it");
        assert_eq!(record.read(), claimed(Some(FX), None));
        record.write(&claimed(Some(FX), Some(CABLE_OUT)));
        assert_eq!(record.read(), claimed(Some(FX), Some(CABLE_OUT)));
        assert!(
            !path.with_extension("toml.tmp").exists(),
            "the temporary file is renamed over the record"
        );
        record.write(&claimed(None, None));
        assert!(!path.exists(), "nothing held, no file");
        assert_eq!(record.read(), claimed(None, None));
        // Removing a record that is not there is no error.
        record.write(&claimed(None, None));
        // The user's own cable defaults alone are something to keep.
        let users = theirs(
            claimed(None, None),
            DeviceDirection::Output,
            Role::Communications,
            CABLE_IN,
        );
        record.write(&users);
        assert_eq!(record.read(), users);
        record.write(&claimed(None, None));
        assert!(!path.exists());
    }

    #[test]
    fn the_user_s_record_is_local_to_the_machine() {
        // Only built: nothing is read, written or listed there.
        let path = Record::of_user()
            .path
            .expect("Windows names a local application data folder");
        assert!(path.ends_with(r"CrossFX\claimed-defaults.toml"), "{path:?}");
        let local = std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA");
        assert!(path.starts_with(local), "{path:?}");
        // Never the original's own folder, which Windows does not tell from `fxsound`.
        assert!(
            !path
                .components()
                .any(|part| part.as_os_str().eq_ignore_ascii_case("fxsound")),
            "{path:?}"
        );
        assert_eq!(
            path.parent(),
            Settings::local_dir().as_deref(),
            "the folder CrossFX keeps for this machine"
        );
    }
}
