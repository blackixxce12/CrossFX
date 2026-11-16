//! Whether the original FxSound — FxSound LLC's, `C:\Program Files\FxSound LLC\FxSound\FxSound.exe`
//! — is running.
//!
//! It holds the same virtual device CrossFX's output lane stands on: it makes "FxSound Speakers"
//! the default, captures it and plays it to the speakers itself. Two programs doing that at once
//! play every sound twice, each through its own chain, and each takes the default back from the
//! other whenever the other moves it (`sndDevicesDeviceCallbacks.cpp:95-156`: the original reads a
//! new default as the user's pick and takes it back). So while it runs the output lane stands
//! aside: it neither takes the default nor processes, and the user is told why
//! ([`crate::ORIGINAL_FXSOUND_RUNNING`]).
//!
//! The original's executable is called `FxSound.exe`; CrossFX's is `crossfx.exe`, so every process
//! called `FxSound.exe`, in any case and wherever it runs, is taken for the original. That is the
//! direction that is safe to be wrong in: standing aside for a program that only shares the name
//! costs the user processing, while fighting the original over the default would cost them their
//! sound.
//!
//! Pure; the process list is `super::com::processes`.

/// The original's executable, compared without regard to case as Windows compares file names.
pub(crate) const IMAGE_NAME: &str = "fxsound.exe";

/// One running process, as the process list gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Process {
    pub(crate) pid: u32,
    /// The executable's file name (`PROCESSENTRY32W::szExeFile`).
    pub(crate) exe: String,
    /// The executable's full path (`QueryFullProcessImageNameW`), when it could be read: for the
    /// log line that names the original, never to tell it from anything.
    pub(crate) image: Option<String>,
}

/// The original FxSound among `processes`, if it runs: the first process called `FxSound.exe`,
/// without regard to case (module docs).
pub(crate) fn find(processes: &[Process]) -> Option<&Process> {
    processes
        .iter()
        .find(|process| process.exe.eq_ignore_ascii_case(IMAGE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGINAL: &str = r"C:\Program Files\FxSound LLC\FxSound\FxSound.exe";
    const CROSSFX: &str = r"C:\Program Files\CrossFX\crossfx.exe";

    fn process(pid: u32, exe: &str, image: Option<&str>) -> Process {
        Process {
            pid,
            exe: exe.to_owned(),
            image: image.map(str::to_owned),
        }
    }

    #[test]
    fn the_original_is_found_among_other_processes() {
        let processes = [
            process(4, "System", None),
            process(100, "crossfx.exe", Some(CROSSFX)),
            process(12_364, "FxSound.exe", Some(ORIGINAL)),
        ];
        let found = find(&processes).expect("the original");
        assert_eq!(found.pid, 12_364);
        assert_eq!(found.image.as_deref(), Some(ORIGINAL));
    }

    #[test]
    fn crossfx_exe_is_never_the_original() {
        let shouted = CROSSFX.to_uppercase();
        let prefixed = format!(r"\\?\{CROSSFX}");
        for (exe, image) in [
            ("crossfx.exe", Some(CROSSFX)),
            ("CROSSFX.EXE", Some(shouted.as_str())),
            ("crossfx.exe", Some(prefixed.as_str())),
            ("crossfx.exe", None),
        ] {
            let processes = [process(4, "System", None), process(100, exe, image)];
            assert_eq!(find(&processes), None, "{exe} {image:?}");
        }
    }

    #[test]
    fn every_fxsound_exe_is_the_original_wherever_it_runs() {
        // Not only the original's folder: whatever is called `FxSound.exe` holds the virtual
        // device the way the original does, or may, and standing aside is the safe direction.
        for (exe, image) in [
            ("FxSound.exe", Some(r"C:\Tools\FxSound.exe")),
            ("FXSOUND.EXE", Some(r"C:\Tools\FXSOUND.EXE")),
            ("fxsound.exe", None),
        ] {
            let processes = [
                process(100, "crossfx.exe", Some(CROSSFX)),
                process(7, exe, image),
            ];
            assert_eq!(
                find(&processes).map(|process| process.pid),
                Some(7),
                "{exe} {image:?}"
            );
        }
    }

    #[test]
    fn a_process_of_another_name_is_not_the_original() {
        for exe in [
            "fxsound-helper.exe",
            "FxSound.exe.bak",
            "notfxsound.exe",
            "FxSound",
        ] {
            assert_eq!(find(&[process(7, exe, None)]), None, "{exe}");
        }
    }
}
