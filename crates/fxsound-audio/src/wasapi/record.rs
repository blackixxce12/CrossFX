//! FxSound's own record of the defaults it holds: the endpoint each lane has made its direction's
//! default, written before the first write of a claim and cleared once its hand-back has moved
//! every role off the endpoint.
//!
//! What the record is for is the run that never hands back. The release profile aborts on a panic
//! and a process can be killed, and either leaves the defaults on the virtual device, which
//! swallows every sound until something gives them back. The next start does (`super`, "The
//! default devices") — but only a default it can tell is FxSound's. A virtual cable is not FxSound's
//! alone: a user who records the desktop's sound may have made "CABLE Output" their default
//! recording device, or play into "CABLE Input" on purpose, and a start that moved those defaults
//! away would break their setup at every start — the input lane, detached until a microphone is
//! picked, never stands behind its cable. So a default left on a cable is given back only when the
//! record says a run of this port put it there. A default on FxSound's own driver needs no record:
//! nothing plays that device while neither FxSound runs. (A lane that comes to stand on a cable
//! later decides for itself whose each default it finds there is — the user's, or one Windows put
//! there by itself: `super`, "Whose default it is".)
//!
//! The Linux engine needs none of this: its virtual nodes are its own, and a default naming one is
//! FxSound's by its name.
//!
//! The file is `%LOCALAPPDATA%\fxsound\claimed-defaults.toml` — local, not roaming, because an
//! endpoint ID names a device of one machine:
//!
//! ```toml
//! output = "{0.0.0.00000000}.{73ca9fe5-1336-4bd0-b0a4-149e64acf557}"
//! ```
//!
//! Written whole, to a temporary file renamed over it, so that a run killed in the middle of a
//! write leaves the old record or the new one, never half of one; removed once neither lane holds
//! anything. Read once, at the start. A record that cannot be read is no record, and a record that
//! cannot be written is logged: neither is worth more than the sound.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::per_direction::PerDirection;

/// The file's name, in [`Record::of_user`]'s directory.
const FILE_NAME: &str = "claimed-defaults.toml";

/// The record's contents: each lane's endpoint, or nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Contents {
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<String>,
}

impl From<&PerDirection<Option<String>>> for Contents {
    fn from(claimed: &PerDirection<Option<String>>) -> Self {
        Self {
            output: claimed.output.clone(),
            input: claimed.input.clone(),
        }
    }
}

/// Where the record is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Record {
    /// `None` when there is nowhere to keep it — no `LOCALAPPDATA` — and then nothing is kept.
    path: Option<PathBuf>,
}

impl Record {
    /// The user's record, in `%LOCALAPPDATA%\fxsound`.
    pub(crate) fn of_user() -> Self {
        Self {
            path: std::env::var_os("LOCALAPPDATA")
                .filter(|dir| !dir.is_empty())
                .map(|dir| PathBuf::from(dir).join("fxsound").join(FILE_NAME)),
        }
    }

    /// A record kept at `path`: a test's own file.
    #[cfg(test)]
    pub(crate) const fn at(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    /// What the record holds; nothing for a record that is not there or cannot be read.
    pub(crate) fn read(&self) -> PerDirection<Option<String>> {
        let Some(path) = self.path.as_ref() else {
            return PerDirection::default();
        };
        match std::fs::read_to_string(path) {
            Ok(text) => parse(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => PerDirection::default(),
            Err(error) => {
                log::warn!("{}: {error}; taken as no record", path.display());
                PerDirection::default()
            }
        }
    }

    /// Keep `claimed`, replacing whatever the record held; remove the file when it holds nothing.
    pub(crate) fn write(&self, claimed: &PerDirection<Option<String>>) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        let written = match render(claimed) {
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
fn parse(text: &str) -> PerDirection<Option<String>> {
    let contents: Contents = toml::from_str(text).unwrap_or_else(|error| {
        log::warn!("the record of FxSound's defaults is unreadable ({error}); taken as no record");
        Contents::default()
    });
    let keep = |id: Option<String>| id.filter(|id| !id.trim().is_empty());
    PerDirection {
        output: keep(contents.output),
        input: keep(contents.input),
    }
}

/// The file `claimed` is kept as, or `None` when it holds nothing and the file goes.
fn render(claimed: &PerDirection<Option<String>>) -> Option<String> {
    let contents = Contents::from(claimed);
    if contents == Contents::default() {
        return None;
    }
    let body = toml::to_string(&contents)
        .map_err(|error| log::warn!("the record of FxSound's defaults: {error}"))
        .ok()?;
    Some(format!(
        "# The default devices FxSound holds, so that a start after a run that was killed gives \
         them back.\n# Written and removed by FxSound itself.\n{body}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FX: &str = "{0.0.0.00000000}.{73ca9fe5-1336-4bd0-b0a4-149e64acf557}";
    const CABLE_OUT: &str = "{0.0.1.00000000}.{cable-out}";

    fn claimed(output: Option<&str>, input: Option<&str>) -> PerDirection<Option<String>> {
        PerDirection {
            output: output.map(str::to_owned),
            input: input.map(str::to_owned),
        }
    }

    #[test]
    fn a_record_reads_back_as_it_was_written_lane_by_lane() {
        for record in [
            claimed(Some(FX), None),
            claimed(None, Some(CABLE_OUT)),
            claimed(Some(FX), Some(CABLE_OUT)),
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
        // Another key beside it is not the record's business.
        assert_eq!(
            parse("output = \"x\"\nlater = true\n"),
            claimed(Some("x"), None)
        );
    }

    #[test]
    fn the_record_is_kept_in_its_file_and_removed_when_it_holds_nothing() {
        let dir = fxsound_core::test_support::ScratchDir::new("wasapi-record");
        let path = dir.path().join("fxsound").join(FILE_NAME);
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
    }

    #[test]
    fn the_user_s_record_is_local_to_the_machine() {
        if let Some(path) = Record::of_user().path {
            assert!(path.ends_with(r"fxsound\claimed-defaults.toml"), "{path:?}");
            let local = std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA");
            assert!(path.starts_with(local), "{path:?}");
        }
    }
}
