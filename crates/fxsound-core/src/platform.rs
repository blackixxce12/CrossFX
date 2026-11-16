//! What the platform this build is for has, decided once, at compile time.
//!
//! CrossFX on Linux sits on PipeWire and WirePlumber, serves D-Bus, and leaves its global
//! shortcuts to the compositor; the same code built for Windows is a WASAPI application with none
//! of those, beside the original FxSound by FxSound LLC. What the interface offers follows from
//! that, and is asked of [`PLATFORM`] rather than of a `cfg` at each control: a pane, a checkbox
//! or a line that one platform lacks names the field it depends on here, so that what Windows does
//! not have is decided in one place and can be read in one place. Linux is described as it was
//! before CrossFX had a Windows build, so nothing there changes.
//!
//! [`Platform::LINUX`] and [`Platform::WINDOWS`] are both ordinary values on both platforms: a
//! test can draw the other platform's interface without building for it.

/// One platform's description.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
    /// The D-Bus service, its methods and its properties, which "Always kept" names.
    pub dbus: bool,
    /// PipeWire's echo canceller in front of the microphone (`module-echo-cancel`): Settings ▸
    /// Microphone ▸ "Echo cancellation" and the line under it.
    pub echo_cancel: bool,
    /// WirePlumber, and CrossFX's hook in it: Settings ▸ Experimental ▸ "Smooth moves in
    /// WirePlumber", the lines under it and "Restart WirePlumber".
    pub wireplumber: bool,
    /// A preset of its own for each application, which moves the application's streams onto a
    /// route of their own (`docs/0.4.0-apps.md`): Settings ▸ Applications. The routed
    /// applications the preset list's tip names are the engine's to report, and an engine without
    /// routes reports none.
    pub app_presets: bool,
    /// Global shortcuts belong to the compositor, which binds CrossFX's commands to keys: the
    /// Wayland note over the shortcut table and the link to the example configuration under it.
    pub compositor_shortcuts: bool,
}

impl Platform {
    /// CrossFX on Linux.
    pub const LINUX: Self = Self {
        dbus: true,
        echo_cancel: true,
        wireplumber: true,
        app_presets: true,
        compositor_shortcuts: true,
    };

    /// CrossFX on Windows: none of the Linux plumbing.
    pub const WINDOWS: Self = Self {
        dbus: false,
        echo_cancel: false,
        wireplumber: false,
        app_presets: false,
        compositor_shortcuts: false,
    };
}

/// The platform this build is for.
pub const PLATFORM: Platform = if cfg!(windows) {
    Platform::WINDOWS
} else {
    Platform::LINUX
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_build_is_described_by_its_own_platform() {
        if cfg!(windows) {
            assert_eq!(PLATFORM, Platform::WINDOWS);
        } else {
            assert_eq!(PLATFORM, Platform::LINUX);
        }
    }

    #[test]
    fn linux_has_all_of_its_plumbing_and_windows_none_of_it() {
        let Platform {
            dbus,
            echo_cancel,
            wireplumber,
            app_presets,
            compositor_shortcuts,
        } = Platform::LINUX;
        assert!(dbus && echo_cancel && wireplumber && app_presets && compositor_shortcuts);

        let Platform {
            dbus,
            echo_cancel,
            wireplumber,
            app_presets,
            compositor_shortcuts,
        } = Platform::WINDOWS;
        assert!(!(dbus || echo_cancel || wireplumber || app_presets || compositor_shortcuts));
    }
}
