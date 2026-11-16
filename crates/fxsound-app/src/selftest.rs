//! `crossfx --self-test [--json]`: is this installation able to run?
//!
//! The question a package's post-install step, a CI leg and a bug report all need answered, and
//! one that "it starts" does not answer: a missing preset directory, a settings file that no
//! longer parses or an unrasterisable tray icon each give a program that starts and then works
//! less than it should. The checks, in order (0.4.0 design §13):
//!
//! 1. the version;
//! 2. where the binary thinks it is installed — `<prefix>/bin/crossfx`, or a cargo `target/`
//!    directory inside the source tree;
//! 3. the factory `.fac` presets and the voice presets, found where the application looks for
//!    them and every one of them parsed;
//! 4. the settings file, parsed **read-only** — `Settings::load` moves a file it cannot parse
//!    aside, which is right for the application and wrong for a test;
//! 5. both processing chains run offline at 48 kHz, the voice chain with RNNoise instantiated and
//!    running, and every sample that comes out finite;
//! 6. the desktop entry, the application and status icons, the systemd user unit, the D-Bus
//!    activation file, the manual page and the AppStream metainfo, under the prefix — `skip` from
//!    the source tree, where they are install-only;
//! 7. the runtime directory the control socket lives in, resolved and usable;
//! 8. PipeWire: `skip` unless `$XDG_RUNTIME_DIR/pipewire-0` exists, and then only whether a server
//!    accepts a connection on it — no protocol is spoken, no node or stream is created, and the
//!    connection is closed straight away;
//! 9. the session bus: `skip` unless `DBUS_SESSION_BUS_ADDRESS` is set.
//!
//! It runs before the single-instance lock, so it works beside a running CrossFX and in a bare
//! container, and it takes no lock, opens no socket of its own, shows no window and writes
//! nothing. The exit status is 1 when any check that ran failed.
//!
//! # On Windows
//!
//! The first five checks are the same, with the presets in the `presets` folder beside
//! `crossfx.exe`, where the installer puts them. Then, in place of the Linux installation's files,
//! its runtime directory, PipeWire and the session bus:
//!
//! 6. `windows_audio`: the Windows audio service (`Audiosrv`) is running — `skip` on a Windows
//!    without one;
//! 7. `virtual_device`: a virtual playback device is there for CrossFX to stand in front of the
//!    speakers with, the FxSound driver's or VB-CABLE's — `skip` without the audio service;
//! 8. `control_pipe`: the control pipe is in use, or free and a pipe of the same security can be
//!    created. Nothing connects to a pipe in use — a running CrossFX would have a caller to
//!    answer — so what holds it is not known here: CrossFX while it runs, or a program that took
//!    the name first and keeps CrossFX from starting, which `crossfx --status` tells apart.
//!
//! Nothing there opens a stream or changes a device; the probe pipe has a name of its own and goes
//! at once.

use fxsound_core::Settings;
use fxsound_core::messages::{DspParams, InputDspParams};
use fxsound_preset::PresetStore;
use fxsound_preset::input::InputPreset;
use serde::Serialize;
use std::io;
#[cfg(target_os = "linux")]
use std::os::unix::fs::FileTypeExt as _;
use std::path::{Path, PathBuf};

/// The rate both chains are run at: the one rate RNNoise exists at.
const RATE: f32 = 48_000.0;
/// Half a second of audio through each chain: enough for RNNoise's 960 frames of latency to pass
/// many times over, and cheap enough to run on every install.
const TEST_FRAMES: usize = 24_000;
/// A block size that is not a multiple of RNNoise's 480-sample frame, so the block bridge is
/// exercised the way a PipeWire quantum exercises it.
const TEST_BLOCK: usize = 256;

/// How one check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    /// Not applicable here — no PipeWire in a container, install-only files in the source tree.
    /// Never a failure.
    Skip,
    Fail,
}

impl Status {
    /// The word the line format prints, which is also the JSON value.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Skip => "skip",
            Self::Fail => "fail",
        }
    }
}

/// One check: its name, how it came out and a one-line detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
}

impl Check {
    fn ok(name: &'static str, detail: impl Into<String>) -> Self {
        Self::new(name, Status::Ok, detail)
    }

    fn skip(name: &'static str, detail: impl Into<String>) -> Self {
        Self::new(name, Status::Skip, detail)
    }

    fn fail(name: &'static str, detail: impl Into<String>) -> Self {
        Self::new(name, Status::Fail, detail)
    }

    fn new(name: &'static str, status: Status, detail: impl Into<String>) -> Self {
        // One line per check, whatever an error message had in it.
        let detail: String = detail.into();
        let detail = detail.split_whitespace().collect::<Vec<_>>().join(" ");
        Self {
            name,
            status,
            detail,
        }
    }
}

/// Every check, and whether the installation passed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub version: &'static str,
    pub checks: Vec<Check>,
    /// `true` when no check failed; skipped ones do not count against it.
    pub ok: bool,
}

impl Report {
    #[must_use]
    pub fn new(checks: Vec<Check>) -> Self {
        let ok = checks.iter().all(|c| c.status != Status::Fail);
        Self {
            version: env!("CARGO_PKG_VERSION"),
            checks,
            ok,
        }
    }

    /// Whether no check failed.
    #[must_use]
    pub const fn ok(&self) -> bool {
        self.ok
    }

    /// The process exit status: 0 when nothing failed, 1 otherwise.
    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        if self.ok { 0 } else { 1 }
    }

    /// `name: ok|skip|fail detail`, one line per check.
    #[must_use]
    pub fn to_text(&self) -> String {
        self.checks
            .iter()
            .map(|check| {
                if check.detail.is_empty() {
                    format!("{}: {}", check.name, check.status.key())
                } else {
                    format!("{}: {} {}", check.name, check.status.key(), check.detail)
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// `{"version":…,"checks":[{"name":…,"status":…,"detail":…}],"ok":…}` on one line.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|err| {
            // A struct of strings and bools has nothing in it that can fail to serialise.
            format!(
                r#"{{"version":"{}","checks":[],"ok":false,"error":"{err}"}}"#,
                self.version
            )
        })
    }

    /// [`Report::to_json`] or [`Report::to_text`].
    #[must_use]
    pub fn render(&self, json: bool) -> String {
        if json { self.to_json() } else { self.to_text() }
    }
}

/// Where the running binary is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Layout {
    /// `<prefix>/bin/crossfx`: `/usr`, `/usr/local`, or wherever a tarball was unpacked. On
    /// Windows, `<prefix>\crossfx.exe`: the folder the installer put it in.
    Installed { prefix: PathBuf },
    /// `<root>/target/<profile>/crossfx`, in a checkout: the presets come from `<root>/assets`
    /// and nothing installed is expected to exist.
    SourceTree { root: PathBuf },
}

impl Layout {
    /// Decide from the binary's own path.
    ///
    /// The source tree is recognised by the preset stores' own rule, `source_tree_of`: the
    /// binary in a checkout's `target/debug` or `target/release` — so the two can never disagree
    /// about whether this is a development build. Anything else is an install, and its prefix is
    /// the directory two above the binary, which is what every package and `install.sh` produce —
    /// on Windows the binary's own, as `fxsound_preset::install_data_dirs` has it.
    #[must_use]
    pub fn of_exe(exe: &Path) -> Self {
        if let Some(root) = fxsound_preset::source_tree_of(exe) {
            return Self::SourceTree { root };
        }
        #[cfg(not(windows))]
        let above = exe.parent().and_then(Path::parent);
        #[cfg(windows)]
        let above = exe.parent();
        Self::Installed {
            prefix: above.map_or_else(|| PathBuf::from("/"), Path::to_path_buf),
        }
    }
}

/// Everything the checks look at, gathered once so a test can point every one of them at a
/// scratch directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Environment {
    /// The binary, as `current_exe()` reports it.
    pub exe: PathBuf,
    pub layout: Layout,
    /// Where the `.fac` store looks for factory presets, in its order.
    pub factory_preset_dirs: Vec<PathBuf>,
    /// Where the voice presets are looked for; the first directory that has any wins.
    pub voice_preset_dirs: Vec<PathBuf>,
    pub settings_path: PathBuf,
    /// Where the lock and the control socket go (`ipc::runtime_dir`).
    #[cfg(target_os = "linux")]
    pub runtime_dir: PathBuf,
    /// `$XDG_RUNTIME_DIR`, where PipeWire's socket is.
    #[cfg(target_os = "linux")]
    pub xdg_runtime_dir: Option<PathBuf>,
    /// `$DBUS_SESSION_BUS_ADDRESS`.
    #[cfg(target_os = "linux")]
    pub dbus_address: Option<String>,
    /// The control pipe's name (`crate::win::pipe`); `None` when the user's SID cannot be read.
    #[cfg(windows)]
    pub control_pipe: Option<PathBuf>,
    /// Whether to ask this machine's audio service and endpoints — never in a test, whose report
    /// must not depend on the host's devices.
    #[cfg(windows)]
    pub probe_audio: bool,
}

impl Environment {
    /// This process's environment: the directories the application itself would use.
    #[must_use]
    pub fn detect() -> Self {
        let exe = std::env::current_exe().unwrap_or_default();
        Self {
            layout: Layout::of_exe(&exe),
            factory_preset_dirs: PresetStore::with_default_dirs().factory_dirs().to_vec(),
            voice_preset_dirs: InputPreset::default_dirs(),
            settings_path: Settings::config_path(),
            #[cfg(target_os = "linux")]
            runtime_dir: crate::ipc::runtime_dir(),
            #[cfg(target_os = "linux")]
            xdg_runtime_dir: std::env::var_os("XDG_RUNTIME_DIR")
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
            #[cfg(target_os = "linux")]
            dbus_address: std::env::var("DBUS_SESSION_BUS_ADDRESS")
                .ok()
                .filter(|address| !address.is_empty()),
            #[cfg(windows)]
            control_pipe: crate::win::pipe::default_path().ok(),
            #[cfg(windows)]
            probe_audio: true,
            exe,
        }
    }

    /// An installation under `prefix`, with its presets where a package puts them, its settings
    /// and runtime directory inside it too, and neither PipeWire nor a session bus — on Windows,
    /// its presets beside `crossfx.exe`, a control pipe of its own, and no audio probed.
    #[must_use]
    pub fn for_prefix(prefix: &Path) -> Self {
        let presets = installed_presets(prefix);
        Self {
            #[cfg(target_os = "linux")]
            exe: prefix.join("bin/crossfx"),
            #[cfg(windows)]
            exe: prefix.join("crossfx.exe"),
            layout: Layout::Installed {
                prefix: prefix.to_path_buf(),
            },
            factory_preset_dirs: vec![presets.join("Factsoft"), presets.join("BonusPresets")],
            voice_preset_dirs: vec![presets.join("Input")],
            settings_path: prefix.join("config/crossfx/settings.toml"),
            #[cfg(target_os = "linux")]
            runtime_dir: prefix.join("run/crossfx"),
            #[cfg(target_os = "linux")]
            xdg_runtime_dir: None,
            #[cfg(target_os = "linux")]
            dbus_address: None,
            #[cfg(windows)]
            control_pipe: crate::win::pipe::path_in(prefix).ok(),
            #[cfg(windows)]
            probe_audio: false,
        }
    }
}

/// Where an installation under `prefix` keeps its shipped presets:
/// `<prefix>/share/crossfx/presets`, or on Windows `<prefix>\presets`, beside the binary.
fn installed_presets(prefix: &Path) -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        prefix.join("share/crossfx/presets")
    }
    #[cfg(windows)]
    {
        prefix.join("presets")
    }
}

/// Run every check.
#[must_use]
pub fn run(env: &Environment) -> Report {
    let mut checks = vec![
        Check::ok("version", env!("CARGO_PKG_VERSION")),
        check_layout(env),
        check_factory_presets(&env.factory_preset_dirs),
        check_voice_presets(&env.voice_preset_dirs),
        check_settings(&env.settings_path),
        check_output_chain(),
        check_input_chain(),
    ];
    #[cfg(target_os = "linux")]
    {
        checks.extend(check_installed_files(env));
        checks.push(check_runtime_dir(&env.runtime_dir));
        checks.push(check_pipewire(env.xdg_runtime_dir.as_deref()));
        checks.push(check_dbus(env.dbus_address.as_deref()));
    }
    #[cfg(windows)]
    {
        let service = env.probe_audio.then(crate::win::audio::service);
        checks.push(check_windows_audio(service.as_ref()));
        let devices = matches!(service, Some(crate::win::audio::Service::Running))
            .then(crate::win::audio::playback_devices);
        checks.push(check_virtual_device(env.probe_audio, devices));
        checks.push(check_control_pipe(env.control_pipe.as_deref()));
    }
    Report::new(checks)
}

/// The Windows audio service, as the service manager reports it; `None` when it was not asked.
#[cfg(windows)]
fn check_windows_audio(service: Option<&crate::win::audio::Service>) -> Check {
    use crate::win::audio::Service;
    const NAME: &str = "windows_audio";
    match service {
        None => Check::skip(NAME, "not probed"),
        Some(Service::Running) => Check::ok(NAME, "Audiosrv is running"),
        Some(Service::NotRunning(state)) => Check::fail(
            NAME,
            format!("Audiosrv is {state}; CrossFX has no sound to process without it"),
        ),
        Some(Service::Missing) => Check::skip(NAME, "this Windows has no audio service"),
        Some(Service::Unknown(why)) => Check::fail(NAME, format!("Audiosrv: {why}")),
    }
}

/// A virtual playback device, from the active endpoints' names; `None` when they were not listed
/// — not probed, or the service is not running.
#[cfg(windows)]
fn check_virtual_device(probed: bool, devices: Option<Result<Vec<String>, String>>) -> Check {
    const NAME: &str = "virtual_device";
    match devices {
        None if probed => Check::skip(NAME, "without the audio service there is nothing to list"),
        None => Check::skip(NAME, "not probed"),
        Some(Err(why)) => Check::fail(
            NAME,
            format!("the playback devices cannot be listed: {why}"),
        ),
        Some(Ok(devices)) => match devices
            .iter()
            .find(|name| crate::win::audio::is_virtual(name))
        {
            Some(device) => Check::ok(NAME, device.clone()),
            None => Check::fail(
                NAME,
                format!(
                    "no virtual playback device among {}: install the FxSound driver or VB-CABLE",
                    if devices.is_empty() {
                        "none".to_owned()
                    } else {
                        devices.join(", ")
                    }
                ),
            ),
        },
    }
}

/// The control pipe: in use, or creatable — a probe instance of the same access control, under a
/// name of its own, made and let go at once.
///
/// A name in use is all this sees of a pipe in use. No connection is made to it, which a running
/// CrossFX would have to answer, and only a connection reads a pipe's owner and label
/// (`win::pipe`'s "Who may connect"): so a program that took the name first — and keeps CrossFX
/// from starting while it holds it — looks here as CrossFX does, and the check says what it found
/// and how to tell the two apart, never that CrossFX is running.
#[cfg(windows)]
fn check_control_pipe(pipe: Option<&Path>) -> Check {
    use crate::win::pipe;
    const NAME: &str = "control_pipe";
    let Some(pipe) = pipe else {
        return Check::fail(
            NAME,
            "the user's SID cannot be read, so the pipe has no name",
        );
    };
    if pipe::exists(pipe) {
        return Check::ok(
            NAME,
            format!(
                "{} (in use: CrossFX's if `crossfx --status` answers; CrossFX cannot start while \
                 another program holds it)",
                pipe.display()
            ),
        );
    }
    let probe = PathBuf::from(format!(
        "{}-self-test-{}",
        pipe.display(),
        std::process::id()
    ));
    match pipe::PipeListener::bind(&probe) {
        Ok(listener) => {
            drop(listener);
            Check::ok(NAME, format!("{} (free)", pipe.display()))
        }
        Err(err) => Check::fail(NAME, format!("{} cannot be created: {err}", pipe.display())),
    }
}

fn check_layout(env: &Environment) -> Check {
    match &env.layout {
        Layout::Installed { prefix } => Check::ok(
            "install_prefix",
            format!("{} ({})", prefix.display(), env.exe.display()),
        ),
        Layout::SourceTree { root } => Check::ok(
            "install_prefix",
            format!("source tree {} ({})", root.display(), env.exe.display()),
        ),
    }
}

/// The files in `dir` with extension `ext` (any case), sorted; `Ok(empty)` for a directory that
/// does not exist, which is the normal state of all but one of the places a preset may be.
fn files_with_extension(dir: &Path, ext: &str) -> io::Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(ext))
        })
        .collect();
    files.sort();
    Ok(files)
}

/// "3 parsed; 1 does not: a.fac: why" or the like, for a list of parse results.
fn parse_failures(bad: &[String]) -> String {
    match bad {
        [] => String::new(),
        [one] => one.clone(),
        [first, rest @ ..] => format!("{first} (and {} more)", rest.len()),
    }
}

/// Every factory directory the `.fac` store searches, every `.fac` in them parsed.
fn check_factory_presets(dirs: &[PathBuf]) -> Check {
    const NAME: &str = "factory_presets";
    let mut parsed = 0;
    let mut bad = Vec::new();
    let mut found_in = Vec::new();
    for dir in dirs {
        let files = match files_with_extension(dir, "fac") {
            Ok(files) => files,
            Err(err) => return Check::fail(NAME, format!("{}: {err}", dir.display())),
        };
        if files.is_empty() {
            continue;
        }
        found_in.push(format!("{} ({})", dir.display(), files.len()));
        for file in files {
            match fxsound_preset::load(&file) {
                Ok(_) => parsed += 1,
                Err(err) => bad.push(format!("{}: {err}", file.display())),
            }
        }
    }
    if !bad.is_empty() {
        Check::fail(
            NAME,
            format!(
                "{} of {} do not parse: {}",
                bad.len(),
                parsed + bad.len(),
                parse_failures(&bad)
            ),
        )
    } else if parsed == 0 {
        Check::fail(NAME, format!("no .fac presets in {}", list(dirs)))
    } else {
        Check::ok(
            NAME,
            format!("{parsed} parsed from {}", found_in.join(", ")),
        )
    }
}

/// The voice presets, from the first directory that has any — the one `InputPreset::load_shipped`
/// takes — every one of them parsed.
fn check_voice_presets(dirs: &[PathBuf]) -> Check {
    const NAME: &str = "voice_presets";
    for dir in dirs {
        let files = match files_with_extension(dir, "toml") {
            Ok(files) => files,
            Err(err) => return Check::fail(NAME, format!("{}: {err}", dir.display())),
        };
        if files.is_empty() {
            continue;
        }
        let total = files.len();
        let bad: Vec<String> = files
            .iter()
            .filter_map(|file| InputPreset::load(file).err().map(|err| format!("{err}")))
            .collect();
        return if bad.is_empty() {
            Check::ok(NAME, format!("{total} parsed from {}", dir.display()))
        } else {
            Check::fail(
                NAME,
                format!(
                    "{} of {total} do not parse: {}",
                    bad.len(),
                    parse_failures(&bad)
                ),
            )
        };
    }
    Check::fail(NAME, format!("no voice presets in {}", list(dirs)))
}

/// Parse the settings file without `Settings::load`, which would move a broken one aside.
fn check_settings(path: &Path) -> Check {
    const NAME: &str = "settings";
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Check::ok(
                NAME,
                format!("{} does not exist yet; the defaults apply", path.display()),
            );
        }
        // A directory is left where it is, and every save fails until someone removes it.
        Err(err) if err.kind() == io::ErrorKind::IsADirectory => {
            return Check::fail(NAME, format!("{}: {err}", path.display()));
        }
        Err(err) => {
            return Check::fail(
                NAME,
                format!(
                    "{}: {err}; CrossFX will move it aside as {} and start from the defaults",
                    path.display(),
                    Settings::next_bad_path(path).display()
                ),
            );
        }
    };
    match toml::from_str::<Settings>(&text) {
        Ok(_) => Check::ok(NAME, path.display().to_string()),
        Err(err) => {
            let line = err
                .span()
                .map(|span| text[..span.start.min(text.len())].lines().count().max(1));
            let at = line.map_or_else(String::new, |line| format!(" line {line}:"));
            Check::fail(
                NAME,
                format!(
                    "{}:{at} {}; CrossFX will move it aside as {} and start from the defaults",
                    path.display(),
                    err.message(),
                    Settings::next_bad_path(path).display()
                ),
            )
        }
    }
}

/// A deterministic test signal: a 440 Hz and a 3 kHz tone with a little noise, interleaved
/// stereo, the right channel a little quieter so the two are not identical.
fn test_signal(frames: usize, start: usize, out: &mut [f32]) {
    let mut seed = 0x2545_f491_u32.wrapping_add(start as u32);
    for (frame, pair) in out
        .as_chunks_mut::<2>()
        .0
        .iter_mut()
        .take(frames)
        .enumerate()
    {
        let t = (start + frame) as f32 / RATE;
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
        let tone = 0.25 * (std::f32::consts::TAU * 440.0 * t).sin()
            + 0.1 * (std::f32::consts::TAU * 3_000.0 * t).sin();
        let sample = tone + 0.02 * noise;
        pair[0] = sample;
        pair[1] = 0.8 * sample;
    }
}

/// Push [`TEST_FRAMES`] of the test signal through `process`, block by block. Returns the output
/// peak, or the frame at which a sample came out that was not finite.
fn run_offline(mut process: impl FnMut(&mut [f32])) -> Result<f32, usize> {
    let mut block = vec![0.0_f32; TEST_BLOCK * 2];
    let mut peak = 0.0_f32;
    let mut done = 0;
    while done < TEST_FRAMES {
        let frames = TEST_BLOCK.min(TEST_FRAMES - done);
        let buffer = &mut block[..frames * 2];
        test_signal(frames, done, buffer);
        process(buffer);
        if let Some(bad) = buffer.iter().position(|s| !s.is_finite()) {
            return Err(done + bad / 2);
        }
        peak = buffer.iter().fold(peak, |peak, s| peak.max(s.abs()));
        done += frames;
    }
    Ok(peak)
}

fn peak_dbfs(peak: f32) -> String {
    if peak > 0.0 {
        format!("{:.1} dBFS", 20.0 * peak.log10())
    } else {
        "silence".to_owned()
    }
}

/// The music chain, every effect and the equalizer engaged.
fn check_output_chain() -> Check {
    const NAME: &str = "output_chain";
    let mut engine = fxsound_dsp::Engine::new(RATE, TEST_BLOCK, 2);
    let mut params = DspParams {
        power: true,
        eq_on: true,
        ..DspParams::default()
    };
    for effect in fxsound_core::Effect::ALL {
        params.set_effect(effect, 0.5);
    }
    params.band_boost_db[0] = 3.0;
    params.band_boost_db[params.band_boost_db.len() / 2] = -3.0;
    engine.apply(&params);
    match run_offline(|buffer| engine.process(buffer, 2)) {
        Err(frame) => Check::fail(
            NAME,
            format!("a sample that is not finite at frame {frame}"),
        ),
        // Five effects at half and a boosted equalizer on two tones cannot come out silent; if
        // they did, the chain is broken in a way the finiteness check alone would pass.
        Ok(peak) if peak <= 0.0 => Check::fail(NAME, "the chain turned a signal into silence"),
        Ok(peak) => Check::ok(
            NAME,
            format!(
                "{TEST_FRAMES} frames at 48 kHz through the equalizer and five effects, peak {}",
                peak_dbfs(peak)
            ),
        ),
    }
}

/// The voice chain with RNNoise on, which is the part of the build most likely to be missing
/// something: the network weights are compiled in, and a chain that cannot run them says so here
/// rather than on the first call.
fn check_input_chain() -> Check {
    const NAME: &str = "input_chain";
    let mut engine = fxsound_dsp::InputEngine::new(RATE, TEST_BLOCK, 2);
    let level = fxsound_core::DenoiseLevel::Medium;
    let params = InputDspParams {
        power: true,
        rnnoise: true,
        denoise_level: level,
        denoise_control: level.control(),
        ..InputDspParams::default()
    };
    engine.apply(&params);
    match run_offline(|buffer| engine.process(buffer, 2)) {
        Err(frame) => Check::fail(
            NAME,
            format!("a sample that is not finite at frame {frame}"),
        ),
        Ok(_) if !engine.denoiser_running() => {
            Check::fail(NAME, "RNNoise was switched on at 48 kHz and did not run")
        }
        Ok(peak) => Check::ok(
            NAME,
            format!(
                "{TEST_FRAMES} frames at 48 kHz with RNNoise running, latency {} frames, peak {}",
                engine.latency_frames(),
                peak_dbfs(peak)
            ),
        ),
    }
}

/// Where a package puts the install-only files, relative to the prefix.
#[cfg(target_os = "linux")]
const DESKTOP_FILE: &str = "share/applications/io.github.blackixxce12.CrossFX.desktop";
#[cfg(target_os = "linux")]
const METAINFO: &str = "share/metainfo/io.github.blackixxce12.CrossFX.metainfo.xml";
#[cfg(target_os = "linux")]
const SYSTEMD_UNIT: &str = "lib/systemd/user/crossfx.service";
#[cfg(target_os = "linux")]
const DBUS_SERVICE: &str = "share/dbus-1/services/io.github.blackixxce12.CrossFX.service";
#[cfg(target_os = "linux")]
const MAN_DIR: &str = "share/man/man1";

/// The icons the tray asks the theme for by name, as a package installs them: the names are the
/// tray's own, so a renamed state is a missing icon here rather than a blank tray on a user's
/// panel.
#[cfg(target_os = "linux")]
fn status_icons() -> Vec<String> {
    [
        crate::tray::ICON_OFF,
        crate::tray::ICON_ON,
        crate::tray::ICON_PROCESSING,
    ]
    .iter()
    .map(|name| format!("share/icons/hicolor/scalable/status/{name}.svg"))
    .collect()
}

/// The application icon at the two sizes every package installs, under its one name: the
/// application id, which the desktop file's `Icon=` names and every notification is sent with.
#[cfg(target_os = "linux")]
fn app_icons() -> Vec<String> {
    ["256x256", "32x32"]
        .iter()
        .map(|size| {
            format!(
                "share/icons/hicolor/{size}/apps/{}.png",
                crate::tray::APP_ID
            )
        })
        .collect()
}

/// The install-only files: under an install prefix, each one present (and, where it is text, what
/// it claims to be); from the source tree, `skip`.
#[cfg(target_os = "linux")]
fn check_installed_files(env: &Environment) -> Vec<Check> {
    let prefix = match &env.layout {
        Layout::Installed { prefix } => prefix,
        Layout::SourceTree { .. } => {
            return [
                "desktop_file",
                "app_icons",
                "status_icons",
                "systemd_unit",
                "dbus_service",
                "man_page",
                "metainfo",
            ]
            .into_iter()
            .map(|name| Check::skip(name, "installed by a package; not in the source tree"))
            .collect();
        }
    };
    vec![
        check_text_file(
            "desktop_file",
            &prefix.join(DESKTOP_FILE),
            "[Desktop Entry]",
        ),
        check_present("app_icons", prefix, &app_icons()),
        check_present("status_icons", prefix, &status_icons()),
        check_unit(&prefix.join(SYSTEMD_UNIT), &prefix.join("bin/crossfx")),
        check_dbus_service(&prefix.join(DBUS_SERVICE), &prefix.join("bin/crossfx")),
        check_man_page(&prefix.join(MAN_DIR)),
        check_text_file("metainfo", &prefix.join(METAINFO), "<component"),
    ]
}

/// Every file in `relative` exists under `prefix` and is not empty.
#[cfg(target_os = "linux")]
fn check_present(name: &'static str, prefix: &Path, relative: &[String]) -> Check {
    let missing: Vec<String> = relative
        .iter()
        .map(|file| prefix.join(file))
        .filter(|path| !std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() > 0))
        .map(|path| path.display().to_string())
        .collect();
    if missing.is_empty() {
        let dirs: Vec<String> = relative
            .iter()
            .filter_map(|file| Path::new(file).parent())
            .map(|dir| prefix.join(dir).display().to_string())
            .fold(Vec::new(), |mut dirs, dir| {
                if !dirs.contains(&dir) {
                    dirs.push(dir);
                }
                dirs
            });
        Check::ok(name, format!("{} in {}", relative.len(), dirs.join(", ")))
    } else {
        Check::fail(name, format!("missing: {}", missing.join(", ")))
    }
}

/// A text file that exists and contains `marker`.
#[cfg(target_os = "linux")]
fn check_text_file(name: &'static str, path: &Path, marker: &str) -> Check {
    match std::fs::read_to_string(path) {
        Ok(text) if text.contains(marker) => Check::ok(name, path.display().to_string()),
        Ok(_) => Check::fail(name, format!("{} has no {marker}", path.display())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            Check::fail(name, format!("missing: {}", path.display()))
        }
        Err(err) => Check::fail(name, format!("{}: {err}", path.display())),
    }
}

/// The systemd user unit, and that it starts *this* binary: the unit ships naming
/// `/usr/bin/crossfx`, and an install anywhere else has to have rewritten it.
#[cfg(target_os = "linux")]
fn check_unit(path: &Path, binary: &Path) -> Check {
    const NAME: &str = "systemd_unit";
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Check::fail(NAME, format!("missing: {}", path.display()));
        }
        Err(err) => return Check::fail(NAME, format!("{}: {err}", path.display())),
    };
    let exec = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("ExecStart="))
        .and_then(|command| command.split_whitespace().next());
    match exec {
        Some(exec) if Path::new(exec) == binary => Check::ok(NAME, path.display().to_string()),
        Some(exec) => Check::fail(
            NAME,
            format!(
                "{} starts {exec}, not this binary ({})",
                path.display(),
                binary.display()
            ),
        ),
        None => Check::fail(NAME, format!("{} has no ExecStart", path.display())),
    }
}

/// The D-Bus activation file, which starts CrossFX for a call to [`crate::dbus::BUS_NAME`]: for
/// that name, handing the start to the user unit as the bus does where it is systemd's, and
/// starting *this* binary by its own `Exec=` where it is not — the file ships naming
/// `/usr/bin/crossfx`, as the unit does.
#[cfg(target_os = "linux")]
fn check_dbus_service(path: &Path, binary: &Path) -> Check {
    const NAME: &str = "dbus_service";
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Check::fail(NAME, format!("missing: {}", path.display()));
        }
        Err(err) => return Check::fail(NAME, format!("{}: {err}", path.display())),
    };
    let value = |key: &str| {
        text.lines().find_map(|line| {
            let (name, value) = line.split_once('=')?;
            (name.trim() == key).then(|| value.trim())
        })
    };
    let at = path.display();
    let unit = SYSTEMD_UNIT.rsplit('/').next().unwrap_or(SYSTEMD_UNIT);
    if !text.lines().any(|line| line.trim() == "[D-BUS Service]") {
        return Check::fail(NAME, format!("{at} has no [D-BUS Service]"));
    }
    match (value("Name"), value("Exec"), value("SystemdService")) {
        (Some(name), _, _) if name != crate::dbus::BUS_NAME => Check::fail(
            NAME,
            format!("{at} is for {name}, not {}", crate::dbus::BUS_NAME),
        ),
        (None, _, _) => Check::fail(NAME, format!("{at} has no Name")),
        (_, None, _) => Check::fail(NAME, format!("{at} has no Exec")),
        (_, Some(exec), _)
            if exec
                .split_whitespace()
                .next()
                .is_none_or(|exec| Path::new(exec) != binary) =>
        {
            Check::fail(
                NAME,
                format!("{at} starts {exec}, not this binary ({})", binary.display()),
            )
        }
        (_, _, Some(systemd)) if systemd == unit => Check::ok(NAME, at.to_string()),
        (_, _, systemd) => Check::fail(
            NAME,
            format!(
                "{at} hands the start to {}, not {unit}",
                systemd.unwrap_or("no unit")
            ),
        ),
    }
}

/// `crossfx.1`, compressed or not — every distribution but the tarball compresses it (gzip for
/// Debian, Ubuntu and Arch, others may use zstd, xz or bzip2), so any `crossfx.1.<suffix>` counts.
///
/// A page that is not there is a failure even where the system chose that: the minimal images of
/// Ubuntu (dpkg `path-exclude`), Arch (pacman `NoExtract`) and Fedora (dnf `tsflags=nodocs`) drop
/// every manual page a package ships. The detail says so, because from inside such an image a
/// correct package and a package without its page look the same.
#[cfg(target_os = "linux")]
fn check_man_page(dir: &Path) -> Check {
    const NAME: &str = "man_page";
    let found = std::fs::read_dir(dir).ok().and_then(|entries| {
        entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name == "crossfx.1" || name.starts_with("crossfx.1."))
            })
    });
    match found {
        Some(path) => Check::ok(NAME, path.display().to_string()),
        None => Check::fail(
            NAME,
            format!(
                "missing: no crossfx.1 or crossfx.1.* in {} (a system set not to install manual \
                 pages — dpkg path-exclude, pacman NoExtract, dnf tsflags=nodocs, as container \
                 images are — has none)",
                dir.display()
            ),
        ),
    }
}

/// The size of `sockaddr_un::sun_path` on Linux, the terminating NUL included: a socket path of
/// this many bytes or more cannot be bound or connected to.
#[cfg(target_os = "linux")]
const SUN_PATH_LEN: usize = 108;

/// The directory the lock and the control socket go in: there already and a directory, or
/// creatable because its parent is, and shallow enough for the socket's path to fit in a Unix
/// socket address. Nothing is created here.
#[cfg(target_os = "linux")]
fn check_runtime_dir(dir: &Path) -> Check {
    const NAME: &str = "runtime_dir";
    // A deep `XDG_RUNTIME_DIR` — a CI scratch directory, a test harness's — gives a directory that
    // is perfectly usable for the lock and a control socket that cannot be claimed.
    let socket = dir.join(crate::ipc::SOCKET_NAME);
    let socket_len = socket.as_os_str().len();
    if socket_len >= SUN_PATH_LEN {
        return Check::fail(
            NAME,
            format!(
                "the control socket {} would be {socket_len} bytes long, and a Unix socket path \
                 has to be shorter than {SUN_PATH_LEN}",
                socket.display()
            ),
        );
    }
    match std::fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_symlink() => Check::fail(
            NAME,
            format!(
                "{} is a symlink, which CrossFX refuses to use",
                dir.display()
            ),
        ),
        Ok(meta) if meta.is_dir() => Check::ok(NAME, dir.display().to_string()),
        Ok(_) => Check::fail(
            NAME,
            format!("{} exists and is not a directory", dir.display()),
        ),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            match dir.parent().map(std::fs::metadata) {
                Some(Ok(parent)) if parent.is_dir() && !parent.permissions().readonly() => {
                    Check::ok(NAME, format!("{} (created at start)", dir.display()))
                }
                Some(Ok(_)) => Check::fail(
                    NAME,
                    format!(
                        "{} cannot be created: its parent is not a writable directory",
                        dir.display()
                    ),
                ),
                _ => Check::fail(
                    NAME,
                    format!(
                        "{} cannot be created: its parent does not exist",
                        dir.display()
                    ),
                ),
            }
        }
        Err(err) => Check::fail(NAME, format!("{}: {err}", dir.display())),
    }
}

/// Whether something is listening on a Unix socket, by connecting and hanging up at once. Enough
/// to tell a live server from a socket file a crashed one left behind; nothing is said on it.
#[cfg(target_os = "linux")]
fn socket_answers(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => {}
        Ok(_) => return Err(format!("{} is not a socket", path.display())),
        Err(err) => return Err(format!("{}: {err}", path.display())),
    }
    std::os::unix::net::UnixStream::connect(path)
        .map(drop)
        .map_err(|err| format!("{}: nothing answers ({err})", path.display()))
}

/// PipeWire: preflight only. A socket that accepts a connection is all this asks. No protocol is
/// spoken and no node or stream is created; the connection is closed as soon as it is accepted.
#[cfg(target_os = "linux")]
fn check_pipewire(xdg_runtime_dir: Option<&Path>) -> Check {
    const NAME: &str = "pipewire";
    let Some(dir) = xdg_runtime_dir else {
        return Check::skip(NAME, "XDG_RUNTIME_DIR is not set");
    };
    let socket = dir.join("pipewire-0");
    if std::fs::symlink_metadata(&socket).is_err() {
        return Check::skip(NAME, format!("no {}", socket.display()));
    }
    match socket_answers(&socket) {
        Ok(()) => Check::ok(NAME, format!("{} answers", socket.display())),
        Err(why) => Check::fail(NAME, why),
    }
}

/// The session bus the tray, the notifications and the D-Bus interface use. A `unix:path=`
/// address is probed like PipeWire's socket; any other transport is reported as set and left
/// alone.
#[cfg(target_os = "linux")]
fn check_dbus(address: Option<&str>) -> Check {
    const NAME: &str = "dbus";
    let Some(address) = address else {
        return Check::skip(NAME, "DBUS_SESSION_BUS_ADDRESS is not set");
    };
    // `unix:path=/run/user/1000/bus[,guid=…]`, possibly one of several `;`-separated addresses.
    let path = address.split(';').find_map(|one| {
        one.strip_prefix("unix:")?
            .split(',')
            .find_map(|kv| kv.strip_prefix("path="))
            .map(PathBuf::from)
    });
    match path {
        Some(path) => match socket_answers(&path) {
            Ok(()) => Check::ok(NAME, format!("{} answers", path.display())),
            Err(why) => Check::fail(NAME, why),
        },
        None => Check::ok(NAME, format!("{address} (not probed)")),
    }
}

fn list(dirs: &[PathBuf]) -> String {
    dirs.iter()
        .map(|dir| dir.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::Value;
    use std::fs;

    /// The repository's own presets, to install into a fake prefix.
    fn assets() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/presets")
    }

    fn copy_dir(from: &Path, to: &Path, ext: &str) -> usize {
        fs::create_dir_all(to).expect("create");
        let mut copied = 0;
        for file in files_with_extension(from, ext).expect("read the assets") {
            fs::copy(&file, to.join(file.file_name().expect("a file name"))).expect("copy");
            copied += 1;
        }
        copied
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().expect("a parent")).expect("create");
        fs::write(path, text).expect("write");
    }

    /// A complete installation under a fresh temporary prefix, as a package lays it out. Also
    /// what the command path's tests run the self-test against, so that none of them looks at
    /// this host's installation, PipeWire socket or session bus.
    pub(crate) fn installed() -> (tempfile::TempDir, Environment) {
        let tmp = tempfile::tempdir().expect("a temporary directory");
        let prefix = tmp.path();
        let presets = installed_presets(prefix);
        for (dir, ext) in [
            ("Factsoft", "fac"),
            ("BonusPresets", "fac"),
            ("Input", "toml"),
        ] {
            assert!(copy_dir(&assets().join(dir), &presets.join(dir), ext) > 0);
        }
        #[cfg(target_os = "linux")]
        install_the_desktop_files(prefix);
        let env = Environment::for_prefix(prefix);
        (tmp, env)
    }

    /// What a Linux package installs beside the presets.
    #[cfg(target_os = "linux")]
    fn install_the_desktop_files(prefix: &Path) {
        write(
            &prefix.join(DESKTOP_FILE),
            "[Desktop Entry]\nExec=crossfx\n",
        );
        for icon in app_icons().iter().chain(&status_icons()) {
            write(&prefix.join(icon), "icon");
        }
        write(
            &prefix.join(SYSTEMD_UNIT),
            &format!(
                "[Service]\nExecStart={}/bin/crossfx --hide\n",
                prefix.display()
            ),
        );
        write(
            &prefix.join(DBUS_SERVICE),
            &format!(
                "[D-BUS Service]\nName=io.github.blackixxce12.CrossFX\nExec={}/bin/crossfx --hide\n\
                 SystemdService=crossfx.service\n",
                prefix.display()
            ),
        );
        write(&prefix.join(MAN_DIR).join("crossfx.1.gz"), "man");
        write(
            &prefix.join(METAINFO),
            "<?xml?>\n<component type=\"desktop\">",
        );
        fs::create_dir_all(prefix.join("run")).expect("create");
    }

    fn check<'a>(report: &'a Report, name: &str) -> &'a Check {
        report
            .checks
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("no {name} check in {report:?}"))
    }

    fn statuses(report: &Report) -> Vec<(&'static str, Status)> {
        report.checks.iter().map(|c| (c.name, c.status)).collect()
    }

    #[test]
    fn a_complete_installation_passes_every_check_that_runs() {
        let (_tmp, env) = installed();
        let report = run(&env);
        assert!(report.ok(), "{}", report.to_text());
        assert_eq!(report.exit_code(), 0);
        for (name, status) in statuses(&report) {
            // Neither PipeWire and the bus nor, on Windows, the host's audio is probed here.
            let expected = if matches!(
                name,
                "pipewire" | "dbus" | "windows_audio" | "virtual_device"
            ) {
                Status::Skip
            } else {
                Status::Ok
            };
            assert_eq!(status, expected, "{name}: {}", report.to_text());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_checks_come_in_a_fixed_order_with_stable_names() {
        let (_tmp, env) = installed();
        let names: Vec<&str> = run(&env).checks.iter().map(|c| c.name).collect();
        assert_eq!(
            names,
            [
                "version",
                "install_prefix",
                "factory_presets",
                "voice_presets",
                "settings",
                "output_chain",
                "input_chain",
                "desktop_file",
                "app_icons",
                "status_icons",
                "systemd_unit",
                "dbus_service",
                "man_page",
                "metainfo",
                "runtime_dir",
                "pipewire",
                "dbus",
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn on_windows_the_checks_come_in_a_fixed_order_with_stable_names() {
        let (_tmp, env) = installed();
        let names: Vec<&str> = run(&env).checks.iter().map(|c| c.name).collect();
        assert_eq!(
            names,
            [
                "version",
                "install_prefix",
                "factory_presets",
                "voice_presets",
                "settings",
                "output_chain",
                "input_chain",
                "windows_audio",
                "virtual_device",
                "control_pipe",
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn the_audio_checks_say_what_is_missing_and_how_to_get_it() {
        use crate::win::audio::Service;
        assert_eq!(
            check_windows_audio(Some(&Service::Running)).status,
            Status::Ok
        );
        assert_eq!(
            check_windows_audio(Some(&Service::NotRunning("stopped"))).status,
            Status::Fail
        );
        assert_eq!(
            check_windows_audio(Some(&Service::Missing)).status,
            Status::Skip
        );
        assert_eq!(check_windows_audio(None).status, Status::Skip);

        let speakers = || Some(Ok(vec!["Speakers (Realtek(R) Audio)".to_owned()]));
        let missing = check_virtual_device(true, speakers());
        assert_eq!(missing.status, Status::Fail);
        assert!(missing.detail.contains("VB-CABLE"), "{}", missing.detail);
        assert!(missing.detail.contains("Realtek"), "{}", missing.detail);
        let found = check_virtual_device(
            true,
            Some(Ok(vec![
                "Speakers (Realtek(R) Audio)".to_owned(),
                "CABLE Input (VB-Audio Virtual Cable)".to_owned(),
            ])),
        );
        assert_eq!(found.status, Status::Ok);
        assert_eq!(found.detail, "CABLE Input (VB-Audio Virtual Cable)");
        assert_eq!(check_virtual_device(true, None).status, Status::Skip);
        assert_eq!(
            check_virtual_device(true, Some(Err("no COM".to_owned()))).status,
            Status::Fail
        );
    }

    #[cfg(windows)]
    #[test]
    fn the_control_pipe_is_free_until_an_instance_holds_it_and_is_never_connected_to() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let pipe = crate::win::pipe::path_in(tmp.path()).expect("a name");
        let free = check_control_pipe(Some(&pipe));
        assert_eq!(free.status, Status::Ok, "{}", free.detail);
        assert!(free.detail.ends_with("(free)"), "{}", free.detail);

        let crate::ipc::Instance::Primary(listener) =
            crate::ipc::Instance::acquire_in(tmp.path()).expect("acquire")
        else {
            panic!("the pipe was free");
        };
        let server = listener.serve().expect("serve");
        let running = check_control_pipe(Some(&pipe));
        assert_eq!(running.status, Status::Ok, "{}", running.detail);
        assert!(running.detail.contains("(in use:"), "{}", running.detail);
        assert!(server.try_recv().is_none(), "nothing reached the instance");
        assert_eq!(check_control_pipe(None).status, Status::Fail);
    }

    /// The name squatted by a sandboxed process of this user, which keeps CrossFX from starting:
    /// what the check sees of it is what it sees of a running CrossFX, a name in use, so it says
    /// that and how to tell the two apart — never that CrossFX is running.
    #[cfg(windows)]
    #[test]
    fn a_control_pipe_squatted_by_another_program_is_not_taken_for_a_running_crossfx() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let pipe = crate::win::pipe::path_in(tmp.path()).expect("a name");
        let _squatter = crate::win::pipe::squat(&pipe).expect("the squatter's pipe");
        assert!(
            matches!(
                crate::ipc::Instance::acquire_in(tmp.path()),
                Ok(crate::ipc::Instance::Secondary(_))
            ),
            "CrossFX would be the second of two, with nobody to talk to"
        );
        let held = check_control_pipe(Some(&pipe));
        assert_eq!(held.status, Status::Ok, "{}", held.detail);
        assert!(!held.detail.contains("is running"), "{}", held.detail);
        assert!(held.detail.contains("(in use:"), "{}", held.detail);
        assert!(
            held.detail.contains("`crossfx --status`"),
            "{}",
            held.detail
        );
        assert!(
            held.detail
                .contains("cannot start while another program holds it"),
            "{}",
            held.detail
        );
    }

    #[test]
    fn the_text_report_is_one_name_status_detail_line_per_check() {
        let (_tmp, env) = installed();
        let report = run(&env);
        let text = report.to_text();
        assert_eq!(text.lines().count(), report.checks.len());
        for (line, check) in text.lines().zip(&report.checks) {
            let (name, rest) = line.split_once(": ").expect("`name: status …`");
            assert_eq!(name, check.name);
            let status = rest.split(' ').next().expect("a status");
            assert!(matches!(status, "ok" | "skip" | "fail"), "{line}");
            assert_eq!(status, check.status.key());
        }
        assert!(text.starts_with(&format!("version: ok {}", env!("CARGO_PKG_VERSION"))));
    }

    #[test]
    fn the_json_report_is_one_document_with_version_checks_and_ok() {
        let (_tmp, env) = installed();
        let report = run(&env);
        let json = report.to_json();
        assert_eq!(
            json.lines().count(),
            1,
            "one line, so `| jq` and a line reader both work"
        );
        let value: Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(value["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(value["ok"], true);
        let checks = value["checks"].as_array().expect("an array");
        assert_eq!(checks.len(), report.checks.len());
        for (json, check) in checks.iter().zip(&report.checks) {
            assert_eq!(json["name"], check.name);
            assert_eq!(json["status"], check.status.key());
            assert_eq!(json["detail"], check.detail.as_str());
        }
        assert_eq!(report.render(true), json);
        assert_eq!(report.render(false), report.to_text());
    }

    #[test]
    fn one_failing_check_fails_the_report_and_a_skip_does_not() {
        let skipped = Report::new(vec![Check::ok("a", ""), Check::skip("b", "not here")]);
        assert!(skipped.ok());
        assert_eq!(skipped.exit_code(), 0);
        assert_eq!(skipped.to_text(), "a: ok\nb: skip not here");

        let failed = Report::new(vec![Check::ok("a", ""), Check::fail("b", "broken")]);
        assert!(!failed.ok());
        assert_eq!(failed.exit_code(), 1);
        let value: Value = serde_json::from_str(&failed.to_json()).expect("valid JSON");
        assert_eq!(value["ok"], false);
    }

    #[test]
    fn a_detail_is_kept_to_one_line() {
        let check = Check::fail("x", "first\n  second\tthird");
        assert_eq!(check.detail, "first second third");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_missing_man_page_fails_that_check_alone() {
        let (tmp, env) = installed();
        fs::remove_file(tmp.path().join(MAN_DIR).join("crossfx.1.gz")).expect("remove");
        let report = run(&env);
        assert!(!report.ok());
        assert_eq!(report.exit_code(), 1);
        let failed: Vec<&str> = report
            .checks
            .iter()
            .filter(|c| c.status == Status::Fail)
            .map(|c| c.name)
            .collect();
        assert_eq!(failed, ["man_page"], "{}", report.to_text());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_uncompressed_man_page_counts_as_well() {
        let (tmp, _env) = installed();
        let dir = tmp.path().join(MAN_DIR);
        fs::rename(dir.join("crossfx.1.gz"), dir.join("crossfx.1")).expect("rename");
        assert_eq!(check_man_page(&dir).status, Status::Ok);
        // A page for something else is not ours.
        fs::rename(dir.join("crossfx.1"), dir.join("crossfxx.1")).expect("rename");
        assert_eq!(check_man_page(&dir).status, Status::Fail);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn every_compressed_man_page_counts() {
        let (tmp, _env) = installed();
        let dir = tmp.path().join(MAN_DIR);
        let mut current = dir.join("crossfx.1.gz");
        for name in [
            "crossfx.1.zst",
            "crossfx.1.xz",
            "crossfx.1.bz2",
            "crossfx.1.gz",
        ] {
            let next = dir.join(name);
            fs::rename(&current, &next).expect("rename");
            let check = check_man_page(&dir);
            assert_eq!(check.status, Status::Ok, "{name}: {}", check.detail);
            assert_eq!(check.detail, next.display().to_string());
            current = next;
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_missing_man_page_says_what_was_looked_for() {
        let (tmp, _env) = installed();
        let dir = tmp.path().join(MAN_DIR);
        fs::remove_file(dir.join("crossfx.1.gz")).expect("remove");
        let check = check_man_page(&dir);
        assert_eq!(check.status, Status::Fail);
        assert!(check.detail.starts_with("missing: "), "{}", check.detail);
        assert!(
            check.detail.contains("crossfx.1.*") && check.detail.contains(&*dir.to_string_lossy()),
            "{}",
            check.detail
        );
        // No man1 directory at all, as in an image that installs no manual pages: the same.
        fs::remove_dir(&dir).expect("remove the directory");
        assert_eq!(check_man_page(&dir).status, Status::Fail);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_missing_status_icon_is_named() {
        let (tmp, env) = installed();
        fs::remove_file(tmp.path().join(&status_icons()[2])).expect("remove");
        let report = run(&env);
        let icons = check(&report, "status_icons");
        assert_eq!(icons.status, Status::Fail);
        assert!(
            icons.detail.contains("CrossFX-processing.svg"),
            "{}",
            icons.detail
        );
        assert_eq!(check(&report, "app_icons").status, Status::Ok);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_application_icon_missing_under_the_application_id_is_named() {
        let (tmp, env) = installed();
        let by_id = app_icons()
            .into_iter()
            .find(|icon| icon.contains("32x32/apps/io.github.blackixxce12.CrossFX.png"))
            .expect("the 32x32 icon under the application id is checked");
        fs::remove_file(tmp.path().join(&by_id)).expect("remove");
        let report = run(&env);
        let icons = check(&report, "app_icons");
        assert_eq!(icons.status, Status::Fail);
        assert!(
            icons.detail.contains("io.github.blackixxce12.CrossFX.png"),
            "{}",
            icons.detail
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_empty_icon_file_is_as_good_as_a_missing_one() {
        let (tmp, env) = installed();
        write(&tmp.path().join(&app_icons()[1]), "");
        assert_eq!(check(&run(&env), "app_icons").status, Status::Fail);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_desktop_file_or_metainfo_that_is_not_one_fails() {
        let (tmp, env) = installed();
        write(&tmp.path().join(DESKTOP_FILE), "not a desktop entry");
        fs::remove_file(tmp.path().join(METAINFO)).expect("remove");
        let report = run(&env);
        assert_eq!(check(&report, "desktop_file").status, Status::Fail);
        let metainfo = check(&report, "metainfo");
        assert_eq!(metainfo.status, Status::Fail);
        assert!(
            metainfo.detail.starts_with("missing: "),
            "{}",
            metainfo.detail
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_unit_that_starts_another_binary_fails() {
        // The tarball's install.sh rewrites the unit for its prefix; one that still names
        // /usr/bin/crossfx under /usr/local starts nothing, or the wrong CrossFX.
        let (tmp, env) = installed();
        write(
            &tmp.path().join(SYSTEMD_UNIT),
            "[Service]\nExecStart=/usr/bin/crossfx --hide\n",
        );
        let unit = run(&env)
            .checks
            .into_iter()
            .find(|c| c.name == "systemd_unit")
            .unwrap();
        assert_eq!(unit.status, Status::Fail);
        assert!(
            unit.detail.contains("starts /usr/bin/crossfx"),
            "{}",
            unit.detail
        );

        write(&tmp.path().join(SYSTEMD_UNIT), "[Service]\nType=simple\n");
        assert_eq!(check(&run(&env), "systemd_unit").status, Status::Fail);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_missing_dbus_activation_file_fails_that_check_alone() {
        let (tmp, env) = installed();
        fs::remove_file(tmp.path().join(DBUS_SERVICE)).expect("remove");
        let report = run(&env);
        let service = check(&report, "dbus_service");
        assert_eq!(service.status, Status::Fail);
        assert!(
            service.detail.starts_with("missing: "),
            "{}",
            service.detail
        );
        let failed: Vec<&str> = report
            .checks
            .iter()
            .filter(|c| c.status == Status::Fail)
            .map(|c| c.name)
            .collect();
        assert_eq!(failed, ["dbus_service"]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_dbus_activation_file_for_another_name_binary_or_unit_fails() {
        // The shipped file names /usr/bin/crossfx; the tarball's install.sh rewrites it for its
        // prefix as it rewrites the unit, and one it missed starts the wrong CrossFX, or none.
        let (tmp, env) = installed();
        let path = tmp.path().join(DBUS_SERVICE);
        let binary = format!("{}/bin/crossfx", tmp.path().display());
        for (file, says) in [
            (
                "[D-BUS Service]\nName=io.github.blackixxce12.CrossFX\n\
                 Exec=/usr/bin/crossfx --hide\nSystemdService=crossfx.service\n"
                    .to_owned(),
                "starts /usr/bin/crossfx",
            ),
            (
                format!(
                    "[D-BUS Service]\nName=org.kde.StatusNotifierWatcher\nExec={binary} --hide\n\
                     SystemdService=crossfx.service\n"
                ),
                "is for org.kde.StatusNotifierWatcher",
            ),
            (
                format!(
                    "[D-BUS Service]\nName=io.github.blackixxce12.CrossFX\nExec={binary} --hide\n"
                ),
                "hands the start to no unit",
            ),
            (
                format!(
                    "[D-BUS Service]\nName=io.github.blackixxce12.CrossFX\nExec={binary}\n\
                     SystemdService=pipewire.service\n"
                ),
                "hands the start to pipewire.service",
            ),
            (
                format!("[Desktop Entry]\nName=io.github.blackixxce12.CrossFX\nExec={binary}\n"),
                "has no [D-BUS Service]",
            ),
        ] {
            write(&path, &file);
            let service = check(&run(&env), "dbus_service").clone();
            assert_eq!(service.status, Status::Fail, "{file}");
            assert!(service.detail.contains(says), "{says}: {}", service.detail);
        }
    }

    /// The activation file and the unit as they ship, from `packaging/`.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_shipped_dbus_activation_file_passes_where_it_is_installed_as_it_ships() {
        // packaging/io.github.blackixxce12.CrossFX.service itself, under /usr as every package
        // puts it.
        let shipped = include_str!("../../../packaging/io.github.blackixxce12.CrossFX.service");
        let (tmp, env) = installed();
        let rewritten = shipped.replace(
            "/usr/bin/crossfx",
            &format!("{}/bin/crossfx", tmp.path().display()),
        );
        write(&tmp.path().join(DBUS_SERVICE), &rewritten);
        assert_eq!(check(&run(&env), "dbus_service").status, Status::Ok);
        let unit = include_str!("../../../packaging/crossfx.service");
        assert!(
            unit.lines()
                .any(|line| line == "BusName=io.github.blackixxce12.CrossFX"),
            "the unit the file activates names the bus name it is activated for"
        );
    }

    /// The shell script the shipped unit's `ExecStop=` runs, with CrossFX's path in it as it
    /// ships: the line is `-/bin/sh -c '<script>'`, the `-` letting a stop with no CrossFX to
    /// reach succeed.
    #[cfg(target_os = "linux")]
    fn shipped_stop_script() -> &'static str {
        let unit = include_str!("../../../packaging/crossfx.service");
        let stops: Vec<&str> = unit
            .lines()
            .filter_map(|line| line.strip_prefix("ExecStop="))
            .collect();
        let [stop] = stops.as_slice() else {
            panic!("the unit has one ExecStop, not {stops:?}");
        };
        stop.strip_prefix("-/bin/sh -c '")
            .and_then(|script| script.strip_suffix('\''))
            .unwrap_or_else(|| panic!("ExecStop is not a failure-tolerant /bin/sh -c: {stop}"))
    }

    /// Run the stop script as systemd would, with `$MAINPID` set to `main_pid` or unset, and with
    /// CrossFX replaced by an `echo` of its command line: what it prints is what it would run.
    #[cfg(target_os = "linux")]
    fn run_stop_script(main_pid: Option<&str>) -> String {
        let script = shipped_stop_script().replace("/usr/bin/crossfx", "echo /usr/bin/crossfx");
        let mut shell = std::process::Command::new("/bin/sh");
        shell.arg("-c").arg(script).env_remove("MAINPID");
        if let Some(pid) = main_pid {
            shell.env("MAINPID", pid);
        }
        let output = shell.output().expect("/bin/sh runs");
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_units_stop_leaves_alone_an_instance_its_start_found_running() {
        // `crossfx --activated` exits 0 when CrossFX runs already; systemd still runs ExecStop
        // for that start, with $MAINPID unset, and --quit would then quit the other instance.
        assert_eq!(run_stop_script(None), "");
    }

    /// The stop names `/usr/bin/crossfx`, as `packaging/crossfx.service` ships it.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_units_stop_quits_the_instance_it_started_while_that_runs() {
        assert_eq!(run_stop_script(Some("4242")), "/usr/bin/crossfx --quit\n");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn no_line_of_the_shipped_unit_quits_crossfx_outside_the_guarded_stop() {
        let unit = include_str!("../../../packaging/crossfx.service");
        let quitting: Vec<&str> = unit
            .lines()
            .filter(|line| !line.starts_with('#') && line.contains("--quit"))
            .collect();
        assert_eq!(
            quitting,
            [format!("ExecStop=-/bin/sh -c '{}'", shipped_stop_script())],
            "only the stop sends --quit, and only behind its $MAINPID test"
        );
        assert!(shipped_stop_script().starts_with("[ -z \"$MAINPID\" ] || "));
    }

    #[test]
    fn a_preset_that_does_not_parse_fails_the_presets_and_is_named() {
        let (tmp, env) = installed();
        let factsoft = installed_presets(tmp.path()).join("Factsoft");
        write(&factsoft.join("Broken.fac"), "this is not a preset\n");
        let report = run(&env);
        let presets = check(&report, "factory_presets");
        assert_eq!(presets.status, Status::Fail);
        assert!(presets.detail.contains("Broken.fac"), "{}", presets.detail);
        assert!(presets.detail.starts_with("1 of "), "{}", presets.detail);
    }

    #[test]
    fn the_factory_presets_are_counted_across_every_directory() {
        let (tmp, env) = installed();
        let presets = installed_presets(tmp.path());
        let expected = files_with_extension(&presets.join("Factsoft"), "fac")
            .unwrap()
            .len()
            + files_with_extension(&presets.join("BonusPresets"), "fac")
                .unwrap()
                .len();
        let found = check_factory_presets(&env.factory_preset_dirs);
        assert_eq!(found.status, Status::Ok);
        assert!(
            found
                .detail
                .starts_with(&format!("{expected} parsed from ")),
            "{}",
            found.detail
        );
    }

    #[test]
    fn no_presets_anywhere_is_a_failure_not_an_empty_success() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = vec![tmp.path().join("a"), tmp.path().join("b")];
        let fac = check_factory_presets(&dirs);
        assert_eq!(fac.status, Status::Fail);
        assert!(
            fac.detail.starts_with("no .fac presets in "),
            "{}",
            fac.detail
        );
        let voice = check_voice_presets(&dirs);
        assert_eq!(voice.status, Status::Fail);
        assert!(
            voice.detail.starts_with("no voice presets in "),
            "{}",
            voice.detail
        );
    }

    #[test]
    fn a_voice_preset_that_does_not_parse_fails_the_voice_presets() {
        let (tmp, env) = installed();
        write(
            &installed_presets(tmp.path()).join("Input/Broken.toml"),
            "name = [\n",
        );
        let voice = check(&run(&env), "voice_presets").clone();
        assert_eq!(voice.status, Status::Fail);
        assert!(voice.detail.contains("Broken.toml"), "{}", voice.detail);
    }

    #[test]
    fn the_voice_presets_come_from_the_first_directory_that_has_any() {
        // The rule `InputPreset::load_shipped` follows: the source tree's set shadows an installed
        // one, so a broken file in the shadowed directory is not what the application loads.
        let tmp = tempfile::tempdir().unwrap();
        let first = tmp.path().join("first");
        let second = tmp.path().join("second");
        let count = copy_dir(&assets().join("Input"), &first, "toml");
        write(&second.join("Broken.toml"), "name = [\n");
        let voice = check_voice_presets(&[tmp.path().join("empty"), first.clone(), second]);
        assert_eq!(voice.status, Status::Ok, "{}", voice.detail);
        assert_eq!(
            voice.detail,
            format!("{count} parsed from {}", first.display())
        );
    }

    #[test]
    fn no_settings_file_yet_is_fine() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = check_settings(&tmp.path().join("settings.toml"));
        assert_eq!(settings.status, Status::Ok);
        assert!(
            settings.detail.contains("defaults apply"),
            "{}",
            settings.detail
        );
    }

    #[test]
    fn a_settings_file_that_parses_is_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.toml");
        Settings::default().save_to(&path).expect("save");
        assert_eq!(check_settings(&path).status, Status::Ok);
    }

    #[test]
    fn a_broken_settings_file_fails_with_its_line_and_is_left_where_it_is() {
        // `Settings::load` would move it aside; the self-test writes nothing, so the file a user
        // is asked about is still there to be looked at.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.toml");
        write(&path, "theme_mode = \"dark\"\npower = \"yes\"\n");
        let settings = check_settings(&path);
        assert_eq!(settings.status, Status::Fail);
        assert!(settings.detail.contains("line 2"), "{}", settings.detail);
        assert!(
            settings.detail.contains("settings.toml.bad"),
            "{}",
            settings.detail
        );
        assert!(path.is_file(), "the file was moved");
        assert!(
            !Settings::bad_path(&path).exists(),
            "the file was moved aside"
        );
    }

    #[test]
    fn a_settings_file_that_is_not_utf8_fails_and_names_where_it_will_be_moved() {
        // `Settings::load` moves it aside now, beside any `.bad` already there; the self-test
        // says where, and moves nothing.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.toml");
        fs::write(&path, b"# \xc7\xe2\xf3\xea\npower = false\n").unwrap();
        fs::write(Settings::bad_path(&path), "an earlier one").unwrap();
        let settings = check_settings(&path);
        assert_eq!(settings.status, Status::Fail);
        assert!(
            settings.detail.contains("settings.toml.2.bad"),
            "{}",
            settings.detail
        );
        assert!(path.is_file(), "the file was moved");
        assert!(!tmp.path().join("settings.toml.2.bad").exists());
    }

    #[test]
    fn the_self_test_writes_nothing_under_the_prefix() {
        let (tmp, env) = installed();
        write(&env.settings_path, "power = \"yes\"\n");
        let listing = |root: &Path| {
            let mut all = Vec::new();
            let mut stack = vec![root.to_path_buf()];
            while let Some(dir) = stack.pop() {
                for entry in fs::read_dir(&dir).unwrap().filter_map(Result::ok) {
                    let path = entry.path();
                    let meta = fs::metadata(&path).unwrap();
                    if meta.is_dir() {
                        stack.push(path.clone());
                    }
                    all.push((path, meta.len(), meta.modified().ok()));
                }
            }
            all.sort();
            all
        };
        let before = listing(tmp.path());
        let report = run(&env);
        assert!(!report.ok(), "the broken settings file fails");
        assert_eq!(
            listing(tmp.path()),
            before,
            "the self-test changed the prefix"
        );
        #[cfg(target_os = "linux")]
        assert!(
            !env.runtime_dir.exists(),
            "the runtime directory was created"
        );
    }

    #[test]
    fn the_output_chain_runs_offline_and_comes_out_finite_and_audible() {
        let chain = check_output_chain();
        assert_eq!(chain.status, Status::Ok, "{}", chain.detail);
        assert!(chain.detail.contains("dBFS"), "{}", chain.detail);
    }

    #[test]
    fn the_input_chain_runs_offline_with_rnnoise_running() {
        let chain = check_input_chain();
        assert_eq!(chain.status, Status::Ok, "{}", chain.detail);
        assert!(chain.detail.contains("RNNoise running"), "{}", chain.detail);
        // RNNoise's 960 frames (design §2 & 3), whatever else the chain adds.
        let latency: usize = chain
            .detail
            .split("latency ")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .and_then(|n| n.parse().ok())
            .expect("the detail names the latency");
        assert!(latency >= 960, "{}", chain.detail);
    }

    #[test]
    fn the_offline_runner_reports_the_first_sample_that_is_not_finite() {
        let mut calls = 0;
        let result = run_offline(|buffer| {
            calls += 1;
            if calls == 3 {
                buffer[10] = f32::NAN;
            }
        });
        assert_eq!(result, Err(2 * TEST_BLOCK + 5));
        let peak = run_offline(|buffer| buffer.fill(0.0)).expect("finite");
        assert_eq!(peak, 0.0);
        assert_eq!(peak_dbfs(peak), "silence");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_binary_under_bin_is_installed_under_the_directory_above() {
        assert_eq!(
            Layout::of_exe(Path::new("/usr/bin/crossfx")),
            Layout::Installed {
                prefix: PathBuf::from("/usr")
            }
        );
        assert_eq!(
            Layout::of_exe(Path::new("/usr/local/bin/crossfx")),
            Layout::Installed {
                prefix: PathBuf::from("/usr/local")
            }
        );
    }

    #[test]
    fn a_binary_in_a_cargo_target_directory_is_the_source_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("Cargo.toml"), "[workspace]\n");
        fs::create_dir_all(root.join("assets/presets")).unwrap();
        let exe = root.join("target/release/crossfx");
        assert_eq!(
            Layout::of_exe(&exe),
            Layout::SourceTree {
                root: root.to_path_buf()
            }
        );
        // Without the checkout around it, the same shape is just an odd prefix.
        let bare = tempfile::tempdir().unwrap();
        #[cfg(target_os = "linux")]
        assert_eq!(
            Layout::of_exe(&bare.path().join("target/release/crossfx")),
            Layout::Installed {
                prefix: bare.path().join("target")
            }
        );
        // On Windows, the folder the binary is in.
        #[cfg(windows)]
        assert_eq!(
            Layout::of_exe(&bare.path().join("target/release/crossfx.exe")),
            Layout::Installed {
                prefix: bare.path().join("target/release")
            }
        );
    }

    #[test]
    fn this_checkout_is_recognised_as_the_source_tree() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        assert!(matches!(
            Layout::of_exe(&root.join("target/debug/crossfx")),
            Layout::SourceTree { .. }
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn from_the_source_tree_the_install_only_files_are_skipped() {
        let (_tmp, mut env) = installed();
        env.layout = Layout::SourceTree {
            root: PathBuf::from("/src/crossfx"),
        };
        let report = run(&env);
        for name in [
            "desktop_file",
            "app_icons",
            "status_icons",
            "systemd_unit",
            "dbus_service",
            "man_page",
            "metainfo",
        ] {
            assert_eq!(check(&report, name).status, Status::Skip, "{name}");
        }
        assert!(report.ok(), "{}", report.to_text());
        assert!(
            check(&report, "install_prefix")
                .detail
                .starts_with("source tree ")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_runtime_directory_is_fine_existing_or_creatable_and_not_otherwise() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("crossfx");
        let creatable = check_runtime_dir(&dir);
        assert_eq!(creatable.status, Status::Ok);
        assert!(creatable.detail.contains("created at start"));
        assert!(!dir.exists(), "checking created it");

        fs::create_dir(&dir).unwrap();
        assert_eq!(check_runtime_dir(&dir).status, Status::Ok);

        let file = tmp.path().join("file");
        write(&file, "x");
        assert_eq!(check_runtime_dir(&file).status, Status::Fail);
        assert_eq!(
            check_runtime_dir(&file.join("crossfx")).status,
            Status::Fail
        );
        assert_eq!(
            check_runtime_dir(&tmp.path().join("no/such/crossfx")).status,
            Status::Fail
        );

        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        assert_eq!(check_runtime_dir(&link).status, Status::Fail);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_runtime_directory_too_deep_for_the_socket_fails_exactly_where_binding_it_would() {
        // Seen for real: with `XDG_RUNTIME_DIR` deep in a scratch directory the self-test said
        // `runtime_dir: ok`, and the application then could not claim its control socket.
        let tmp = tempfile::tempdir().unwrap();
        // A directory whose control socket's path is exactly `len` bytes long.
        let dir_for = |len: usize| {
            let fixed = tmp.path().as_os_str().len() + 1 + 1 + crate::ipc::SOCKET_NAME.len();
            tmp.path().join("d".repeat(len - fixed))
        };
        for len in [SUN_PATH_LEN - 1, SUN_PATH_LEN, SUN_PATH_LEN + 40] {
            let dir = dir_for(len);
            fs::create_dir(&dir).unwrap();
            let socket = dir.join(crate::ipc::SOCKET_NAME);
            assert_eq!(socket.as_os_str().len(), len);

            let check = check_runtime_dir(&dir);
            let binds = std::os::unix::net::UnixListener::bind(&socket).is_ok();
            assert_eq!(
                binds,
                len < SUN_PATH_LEN,
                "the limit is not where it was taken to be"
            );
            assert_eq!(
                check.status == Status::Ok,
                binds,
                "{len} bytes: {}",
                check.detail
            );
            if !binds {
                assert!(
                    check.detail.contains(&len.to_string()),
                    "the report should say how long the path is: {}",
                    check.detail
                );
            }
        }

        // A directory that would be created at start, under a parent that is there and writable,
        // is no better for not existing yet.
        let absent = dir_for(SUN_PATH_LEN + 40).join("crossfx");
        let check = check_runtime_dir(&absent);
        assert_eq!(check.status, Status::Fail);
        assert!(check.detail.contains("socket"), "{}", check.detail);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pipewire_is_skipped_without_a_runtime_directory_or_a_socket() {
        assert_eq!(check_pipewire(None).status, Status::Skip);
        let tmp = tempfile::tempdir().unwrap();
        let skipped = check_pipewire(Some(tmp.path()));
        assert_eq!(skipped.status, Status::Skip);
        assert!(skipped.detail.contains("pipewire-0"), "{}", skipped.detail);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pipewire_is_ok_when_something_answers_on_its_socket() {
        let tmp = tempfile::tempdir().unwrap();
        let _server = std::os::unix::net::UnixListener::bind(tmp.path().join("pipewire-0"))
            .expect("bind a stand-in server");
        let check = check_pipewire(Some(tmp.path()));
        assert_eq!(check.status, Status::Ok, "{}", check.detail);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_pipewire_socket_nobody_answers_on_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("pipewire-0");
        // A socket file left behind by a server that is gone.
        drop(std::os::unix::net::UnixListener::bind(&path).expect("bind"));
        assert_eq!(check_pipewire(Some(tmp.path())).status, Status::Fail);

        // And something that is not a socket at all.
        fs::remove_file(&path).unwrap();
        write(&path, "");
        let check = check_pipewire(Some(tmp.path()));
        assert_eq!(check.status, Status::Fail);
        assert!(check.detail.contains("not a socket"), "{}", check.detail);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn dbus_is_skipped_unset_probed_by_path_and_believed_otherwise() {
        assert_eq!(check_dbus(None).status, Status::Skip);

        let tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().join("bus");
        let address = format!("unix:path={},guid=0123", bus.display());
        assert_eq!(
            check_dbus(Some(&address)).status,
            Status::Fail,
            "no socket yet"
        );
        let _server = std::os::unix::net::UnixListener::bind(&bus).expect("bind");
        let ok = check_dbus(Some(&address));
        assert_eq!(ok.status, Status::Ok, "{}", ok.detail);

        // The second of two addresses is found too.
        let two = format!("unix:abstract=/tmp/dbus-x;unix:path={}", bus.display());
        assert_eq!(check_dbus(Some(&two)).status, Status::Ok);

        let abstract_only = check_dbus(Some("unix:abstract=/tmp/dbus-x"));
        assert_eq!(abstract_only.status, Status::Ok);
        assert!(abstract_only.detail.contains("not probed"));
    }

    #[test]
    fn the_detected_environment_uses_the_application_s_own_directories() {
        let env = Environment::detect();
        assert_eq!(
            env.factory_preset_dirs,
            PresetStore::with_default_dirs().factory_dirs()
        );
        assert_eq!(env.voice_preset_dirs, InputPreset::default_dirs());
        assert_eq!(env.settings_path, Settings::config_path());
        #[cfg(target_os = "linux")]
        assert_eq!(env.runtime_dir, crate::ipc::runtime_dir());
        #[cfg(windows)]
        assert_eq!(env.control_pipe, crate::win::pipe::default_path().ok());
        assert_eq!(env.layout, Layout::of_exe(&env.exe));
    }
}
