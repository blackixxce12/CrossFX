//! Notifications on Windows: a toast for each [`Message`], where Linux sends
//! `org.freedesktop.Notifications` a call (`crate::notify`).
//!
//! The semantics are the Linux ones, which are the original's (`notify.rs`'s module docs):
//!
//! - **One slot.** Every toast carries the same tag and group, so a new one takes the place of the
//!   one before — in the notification centre and on screen — instead of stacking.
//! - **Silent.** `<audio silent="true"/>`: the original's balloon is `NIIF_NOSOUND`
//!   (`FxSystemTrayView.cpp:411`), and a sound program does not chime over the sound it plays.
//! - **Weight.** An [`Weight::Alert`] pops up; an [`Weight::Echo`] goes to the notification centre
//!   without a banner (`SuppressPopup`), as GNOME files a low-urgency one. Do Not Disturb holds
//!   back either, as it does any application's.
//! - **A link** is followed when the toast is clicked, its body or its button.
//! - **Gone with CrossFX.** What CrossFX showed is taken out of the notification centre when it
//!   quits, as GNOME takes the notices of a sender that left the bus: they would otherwise offer
//!   a link of an application that is not running.
//!
//! # Who the toast is from
//!
//! Windows shows a toast only for an application it knows by its AppUserModelID. A packaged app
//! has one from its package; this one is registered under
//! `HKCU\Software\Classes\AppUserModelId\io.github.blackixxce12.CrossFX` ([`APP_USER_MODEL_ID`])
//! before the first toast, with the name and the icon the toast is shown with — the way the
//! Windows App SDK registers an unpackaged application for its notifications. The ID and the name
//! are CrossFX's own ([`crossfx_core::identity`]), as its installers' are: FxSound LLC's FxSound
//! may be installed beside it. The process does not take that ID for its window as well: the
//! taskbar would then group the window under it and apart from a pinned shortcut that does not
//! carry it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager, ToastNotifier};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW};
use windows::core::{HSTRING, PCWSTR};

use crate::notify::{APP_NAME, Message, Sink, Weight};

use super::wide;

/// The AppUserModelID the toasts are shown under: CrossFX's application id. Its registration,
/// `HKCU\Software\Classes\AppUserModelId\io.github.blackixxce12.CrossFX`, is the uninstallers' to
/// delete.
pub const APP_USER_MODEL_ID: &str = crossfx_core::identity::APP_ID;

/// The name the toasts are shown under, and the one Windows' notification settings list:
/// CrossFX's, as the Start menu and the installed apps name it.
const DISPLAY_NAME: &str = crossfx_core::identity::PRODUCT;

/// The tag and the group every toast is shown with: the one slot.
const SLOT: &str = crossfx_core::identity::BINARY;

/// Where an unpackaged application's AppUserModelID is described.
const APP_ID_KEY: &str = r"Software\Classes\AppUserModelId";

/// `text` with the five characters XML gives a meaning escaped.
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            c => escaped.push(c),
        }
    }
    escaped
}

/// The toast's XML for `message`.
///
/// A generic toast shows up to three lines of text and the first in bold: the message's first
/// line is that headline — "CrossFX in system tray", "Preset: Rock" — and the rest follow it. The
/// application's own name is in the toast's header already, so it is not the headline as well.
#[must_use]
pub fn content(message: &Message) -> String {
    let body = message.body.replace("\r\n", "\n");
    let mut lines = body.lines().map(str::trim).filter(|line| !line.is_empty());
    let mut texts: Vec<String> = Vec::new();
    texts.extend(lines.next().map(str::to_owned));
    texts.extend(lines.next().map(str::to_owned));
    let rest: Vec<&str> = lines.collect();
    if !rest.is_empty() {
        texts.push(rest.join(" "));
    }
    if texts.is_empty() {
        texts.push(APP_NAME.to_owned());
    }
    let texts: String = texts
        .iter()
        .map(|text| format!("<text>{}</text>", escape(text)))
        .collect();
    let actions = message.link.as_ref().map_or_else(String::new, |link| {
        format!(
            r#"<actions><action content="{}" arguments="link"/></actions>"#,
            escape(&link.label)
        )
    });
    format!(
        r#"<toast duration="short"><visual><binding template="ToastGeneric">{texts}</binding></visual>{actions}<audio silent="true"/></toast>"#
    )
}

/// The registry's description of the AppUserModelID: the name a toast is shown under, and the
/// icon beside it.
fn register_app_id(icon: Option<&PathBuf>) -> windows::core::Result<()> {
    let key = wide(format!(r"{APP_ID_KEY}\{APP_USER_MODEL_ID}"));
    let mut values: Vec<(&str, String)> = vec![("DisplayName", DISPLAY_NAME.to_owned())];
    if let Some(icon) = icon {
        values.push(("IconUri", icon.display().to_string()));
    }
    for (name, value) in values {
        let (name, value) = (wide(name), wide(value));
        let bytes = u32::try_from(value.len() * size_of::<u16>()).unwrap_or(u32::MAX);
        // SAFETY: NUL-terminated strings, and `bytes` of string data, that outlive the call.
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                PCWSTR(key.as_ptr()),
                PCWSTR(name.as_ptr()),
                REG_SZ.0,
                Some(value.as_ptr().cast()),
                bytes,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(windows::core::Error::from_hresult(status.to_hresult()));
        }
    }
    Ok(())
}

/// The icon a toast is shown with: the tray's blue bars, written once as a PNG where the
/// registration can point at it ([`icon_path`]). `None` when it cannot be written; the toast then
/// has Windows' default.
fn toast_icon() -> Option<PathBuf> {
    let path = icon_path()?;
    if path.is_file() {
        return Some(path);
    }
    let png = crate::tray::rasterise_png(crate::tray::TrayIcon::Processing.svg(), 64)?;
    std::fs::create_dir_all(path.parent()?)
        .and_then(|()| std::fs::write(&path, png))
        .map_err(|err| log::debug!("no icon for the notifications: {err}"))
        .ok()?;
    Some(path)
}

/// Where the toasts' icon is kept: `notification.png` in CrossFX's own local folder,
/// `%LOCALAPPDATA%\CrossFX` ([`crossfx_core::Settings::local_dir`]), never the folder of the
/// original FxSound there. The uninstallers delete it, and the folder once it is empty.
fn icon_path() -> Option<PathBuf> {
    crossfx_core::Settings::local_dir().map(|dir| dir.join("notification.png"))
}

/// Delivery as toasts (see the module docs). Replaces `notify::DesktopSink` on Windows.
#[derive(Debug, Default)]
pub struct ToastSink {
    /// Made with the first toast, on the notifier's own thread.
    notifier: Option<ToastNotifier>,
    /// The toast on screen or in the notification centre: kept, because its click handler goes
    /// with it.
    shown: Option<ToastNotification>,
    /// A count, for the id [`Sink::deliver`] hands back: the slot itself is the tag.
    delivered: u32,
}

impl ToastSink {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn notifier(&mut self) -> windows::core::Result<ToastNotifier> {
        if let Some(notifier) = &self.notifier {
            return Ok(notifier.clone());
        }
        static REGISTERED: AtomicBool = AtomicBool::new(false);
        if !REGISTERED.swap(true, Ordering::SeqCst)
            && let Err(err) = register_app_id(toast_icon().as_ref())
        {
            log::warn!(
                "notifications may not show: CrossFX could not register itself for them ({err})"
            );
        }
        let notifier =
            ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_USER_MODEL_ID))?;
        self.notifier = Some(notifier.clone());
        Ok(notifier)
    }

    fn show(&mut self, message: &Message) -> windows::core::Result<()> {
        let document = XmlDocument::new()?;
        document.LoadXml(&HSTRING::from(content(message)))?;
        let toast = ToastNotification::CreateToastNotification(&document)?;
        toast.SetTag(&HSTRING::from(SLOT))?;
        toast.SetGroup(&HSTRING::from(SLOT))?;
        if message.weight == Weight::Echo {
            toast.SetSuppressPopup(true)?;
        }
        if let Some(link) = &message.link {
            let url = link.url.clone();
            toast.Activated(&TypedEventHandler::new(move |_, _| {
                if let Err(err) = crate::platform::open_url(&url) {
                    log::warn!("could not open {url}: {err}");
                }
                Ok(())
            }))?;
        }
        self.notifier()?.Show(&toast)?;
        self.shown = Some(toast);
        Ok(())
    }
}

impl Sink for ToastSink {
    fn deliver(&mut self, message: &Message, _replaces: Option<u32>) -> Option<u32> {
        match self.show(message) {
            Ok(()) => {
                self.delivered = self.delivered.wrapping_add(1);
                Some(self.delivered)
            }
            Err(err) => {
                // A machine with notifications off, or none at all (Server Core): a line in the
                // log, as a session without a notification daemon is on Linux.
                log::warn!("could not show a notification: {err}");
                self.notifier = None;
                None
            }
        }
    }
}

impl Drop for ToastSink {
    fn drop(&mut self) {
        if self.shown.take().is_some()
            && let Ok(history) = ToastNotificationManager::History()
        {
            let _ =
                history.RemoveGroupWithId(&HSTRING::from(SLOT), &HSTRING::from(APP_USER_MODEL_ID));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The toasts are CrossFX's: the AppUserModelID and the name Windows lists them under are
    /// CrossFX's own, and the icon is kept in CrossFX's local folder — never under a name of the
    /// original FxSound's, which Windows does not tell from `fxsound`. Paths are only built here:
    /// nothing is written, read or registered.
    #[test]
    fn the_toasts_are_crossfx_own_and_their_icon_is_kept_in_its_local_folder() {
        assert_eq!(APP_USER_MODEL_ID, "io.github.blackixxce12.CrossFX");
        assert_eq!(DISPLAY_NAME, "CrossFX");
        for name in [APP_USER_MODEL_ID, DISPLAY_NAME, SLOT] {
            assert!(!name.to_lowercase().contains("fxsound"), "{name}");
        }
        let local = dirs::data_local_dir().expect("the local application data");
        let icon = icon_path().expect("a place for the icon");
        assert_eq!(icon, local.join("CrossFX").join("notification.png"));
        for component in icon.components() {
            let component = component.as_os_str().to_string_lossy();
            assert!(
                !component.eq_ignore_ascii_case("fxsound"),
                "{}",
                icon.display()
            );
        }
    }

    #[test]
    fn the_first_line_is_the_headline_and_the_toast_never_makes_a_sound() {
        let xml = content(&Message::minimised_to_tray());
        assert!(
            xml.contains(
                "<text>CrossFX in system tray</text><text>Click CrossFX icon to reopen</text>"
            ),
            "{xml}"
        );
        assert!(xml.contains(r#"<audio silent="true"/>"#), "{xml}");
        assert!(!xml.contains("<actions>"), "no link, no button: {xml}");
    }

    #[test]
    fn what_a_name_holds_is_text_and_never_markup() {
        let xml = content(&Message::preset_selected("Rock & <Roll> \"live\""));
        assert!(
            xml.contains("<text>Preset: Rock &amp; &lt;Roll&gt; &quot;live&quot;</text>"),
            "{xml}"
        );
        let document = XmlDocument::new().expect("a document");
        document
            .LoadXml(&HSTRING::from(xml))
            .expect("the toast's XML parses");
    }

    #[test]
    fn a_link_is_a_button_with_its_label_and_more_than_three_lines_are_folded_into_the_third() {
        let message =
            Message::with_link("One\nTwo\nThree\nFour", "Read it.", "https://example.org/");
        let xml = content(&message);
        assert!(
            xml.contains("<text>One</text><text>Two</text><text>Three Four</text>"),
            "{xml}"
        );
        assert!(
            xml.contains(r#"<action content="Read it." arguments="link"/>"#),
            "{xml}"
        );
    }
}
