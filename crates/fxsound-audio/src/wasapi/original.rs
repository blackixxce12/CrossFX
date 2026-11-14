//! Whether the original FxSound — FxSound LLC's, `C:\Program Files\FxSound LLC\FxSound\FxSound.exe`
//! — is running.
//!
//! It holds the same virtual device this port's output lane stands on: it makes "FxSound Speakers"
//! the default, captures it and plays it to the speakers itself. Two programs doing that at once
//! play every sound twice, each through its own chain, and each takes the default back from the
//! other whenever the other moves it (`sndDevicesDeviceCallbacks.cpp:95-156`: the original reads a
//! new default as the user's pick and takes it back). So while it runs the output lane stands
//! aside: it neither takes the default nor processes, and the user is told why
//! ([`crate::ORIGINAL_FXSOUND_RUNNING`]).
//!
//! The original's executable is called `FxSound.exe` and so is this port's (`fxsound.exe`): the
//! two are told apart by path, as reported by the same call for both, never by name alone. A
//! process of that name whose path cannot be read is taken for the original, which is the
//! direction that is safe to be wrong in: standing aside for nothing costs the user processing,
//! fighting the original over the default costs them their sound.
//!
//! Pure; the process list is `super::com::processes`.

/// The executable both FxSounds are called by, compared without regard to case as Windows
/// compares file names.
pub(crate) const IMAGE_NAME: &str = "fxsound.exe";

/// One running process, as the process list gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Process {
    pub(crate) pid: u32,
    /// The executable's file name (`PROCESSENTRY32W::szExeFile`).
    pub(crate) exe: String,
    /// The executable's full path (`QueryFullProcessImageNameW`), when it could be read.
    pub(crate) image: Option<String>,
}

/// The original FxSound among `processes`, if it runs: a process called `FxSound.exe` that is not
/// this process and whose image is not this process's (`own_image`, read the same way).
pub(crate) fn find<'a>(
    processes: &'a [Process],
    own_pid: u32,
    own_image: Option<&str>,
) -> Option<&'a Process> {
    processes.iter().find(|process| {
        process.pid != own_pid
            && process.exe.to_lowercase() == IMAGE_NAME
            && match (process.image.as_deref(), own_image) {
                (Some(image), Some(own)) => !same_path(image, own),
                // Unreadable: taken for the original (module docs).
                _ => true,
            }
    })
}

/// Whether two paths name the same file, as Windows compares paths: without regard to case, and
/// with or without the `\\?\` prefix a path read one way or another may carry.
fn same_path(a: &str, b: &str) -> bool {
    let plain = |path: &str| path.trim_start_matches(r"\\?\").to_lowercase();
    plain(a) == plain(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGINAL: &str = r"C:\Program Files\FxSound LLC\FxSound\FxSound.exe";
    const OURS: &str = r"C:\Users\Администратор\AppData\Local\Programs\FxSound\fxsound.exe";

    fn process(pid: u32, exe: &str, image: Option<&str>) -> Process {
        Process {
            pid,
            exe: exe.to_owned(),
            image: image.map(str::to_owned),
        }
    }

    #[test]
    fn the_original_is_the_fxsound_exe_that_is_not_this_one() {
        let processes = [
            process(4, "System", None),
            process(100, "fxsound.exe", Some(OURS)),
            process(12_364, "FxSound.exe", Some(ORIGINAL)),
        ];
        let found = find(&processes, 100, Some(OURS)).expect("the original");
        assert_eq!(found.pid, 12_364);
        // Alone, this port finds nothing.
        assert_eq!(find(&processes[..2], 100, Some(OURS)), None);
    }

    #[test]
    fn a_second_copy_of_this_port_is_not_the_original_however_its_path_is_spelt() {
        let shouted = OURS.to_uppercase();
        let prefixed = format!(r"\\?\{OURS}");
        for image in [OURS, shouted.as_str(), prefixed.as_str()] {
            let processes = [process(200, "FXSOUND.EXE", Some(image))];
            assert_eq!(find(&processes, 100, Some(OURS)), None, "{image}");
        }
    }

    #[test]
    fn an_fxsound_exe_whose_path_cannot_be_read_is_taken_for_the_original() {
        let processes = [process(12_364, "FxSound.exe", None)];
        assert!(find(&processes, 100, Some(OURS)).is_some());
        // And so is every other one, when this process cannot read its own path.
        let processes = [process(12_364, "FxSound.exe", Some(ORIGINAL))];
        assert!(find(&processes, 100, None).is_some());
        // A process of another name is nobody's FxSound, path or none.
        assert_eq!(
            find(&[process(7, "fxsound-helper.exe", None)], 100, Some(OURS)),
            None
        );
    }
}
