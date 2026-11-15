//! The CrossFX application: the controller, the command-line surface and the desktop integration.
//!
//! Split into a library so every part of it can be tested without opening a window — the binary in
//! `main.rs` is only the shell that wires these modules to eframe.
//!
//! The desktop integration is the one part that differs between the two systems. On Linux it is
//! the session's: D-Bus (`dbus`, `notify`, `sleep`, the tray), a Unix socket (`ipc`) and signals.
//! On Windows, `win` stands in for each piece behind the same API — the control pipe, the
//! notification area, toasts, the power and session notifications — and [`platform`] is where the
//! rest of the application asks for what both have in their own way: a link or a folder opened,
//! the start at log-in, the desktop's language, the terminal CrossFX was started from.
//!
//! The documentation is built for one system at a time, and of the public items alone: a link
//! goes only to an item that is public and that every system documenting the page has. A private
//! item, or one only the other system has — `dbus` on Windows, `win` on Linux, and what is in
//! them — is named in backticks instead, since a link to it would be broken in the pages
//! `cargo doc` makes. CI builds those pages for each system with rustdoc's warnings denied.

// Linux: no `unsafe` anywhere. Windows: every Win32 call is `unsafe` by signature, so the crate
// denies it rather than forbids it, and `win` alone allows it.
#![cfg_attr(not(windows), forbid(unsafe_code))]
#![cfg_attr(windows, deny(unsafe_code))]

pub mod app;
pub mod audio_link;
pub mod calibration;
pub mod cli;
pub mod commands;
#[cfg(target_os = "linux")]
pub mod dbus;
pub mod events;
pub mod fonts;
pub mod ipc;
pub mod notify;
pub mod platform;
mod priority;
#[cfg(all(test, target_os = "linux"))]
mod private_bus;
pub mod selftest;
#[cfg(target_os = "linux")]
pub mod sleep;
/// Suspend and resume: Windows' power notification in place of logind's signal.
#[cfg(windows)]
pub use win::power as sleep;
pub mod tray;
pub mod wake;
#[cfg(windows)]
pub mod win;

pub use app::{App, WindowVisibility};
pub use commands::{Outcome, WindowRequest};

/// What CrossFX logs when `RUST_LOG` says nothing: its own `info` and up, and only warnings and
/// errors from the D-Bus library and the tracing bridge under it.
///
/// Left at `info` for everything, zbus wrote six lines of its authentication handshake, raw bytes
/// and all, for every connection it opened — the tray's, the service's, and one for every desktop
/// notification — into the terminal or the journal of anyone running CrossFX, about a third of all
/// it logged in the 0.4.0 live check. `RUST_LOG` still sets whatever it likes.
pub const DEFAULT_LOG_FILTER: &str = "info,zbus=warn,tracing=warn";

#[cfg(test)]
mod tests {
    use super::DEFAULT_LOG_FILTER;
    use log::Level;

    #[test]
    fn the_default_log_filter_keeps_the_bus_librarys_handshakes_out_and_crossfx_own_lines_in() {
        let logger = env_logger::Builder::new()
            .parse_filters(DEFAULT_LOG_FILTER)
            .build();
        let at = |target: &str, level: Level| {
            log::Log::enabled(
                &logger,
                &log::Metadata::builder().target(target).level(level).build(),
            )
        };
        assert!(!at("zbus::connection::handshake::common", Level::Info));
        assert!(!at("tracing::span", Level::Info));
        assert!(
            at("zbus::connection", Level::Warn),
            "a bus library's warning still shows"
        );
        // The binary's own target is its crate name, `[[bin]] name`.
        assert!(at("crossfx", Level::Info));
        assert!(at("fxsound_audio::engine", Level::Info));
        assert!(at("fxsound_app::tray", Level::Warn));
        assert!(!at("fxsound_audio::engine", Level::Debug));
    }

    /// `build.rs` cannot name `fxsound_core::identity`, so it spells the names `crossfx.exe`'s
    /// file properties and Task Manager show itself.
    #[test]
    fn the_windows_resources_spell_the_binary_and_the_product_as_identity_does() {
        use fxsound_core::identity::{BINARY, PRODUCT};
        let script = include_str!("../build.rs");
        let constant = |name: &str| {
            let start = format!("const {name}: &str = \"");
            let at = script
                .find(&start)
                .unwrap_or_else(|| panic!("build.rs declares no {name}"))
                + start.len();
            script[at..]
                .split('"')
                .next()
                .unwrap_or_default()
                .to_owned()
        };
        assert_eq!(constant("BINARY"), BINARY);
        assert_eq!(constant("PRODUCT"), PRODUCT);
        assert_eq!(constant("COMPANY"), format!("{PRODUCT} contributors"));
        assert!(
            script.contains(&format!(
                "Copyright \\u{{a9}} {PRODUCT} contributors, portions"
            )),
            "the copyright line names CrossFX's contributors"
        );
    }
}
