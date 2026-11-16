//! The language Windows would show CrossFX in, for the interface's "the system's language".
//!
//! `crossfx_core::i18n::system_language` reads the session's language the POSIX way — `LC_ALL`,
//! then `LC_MESSAGES`, then `LANG` — and so does `crossfx_audio`'s locale. Windows sets none of
//! them, so a Russian Windows got an English CrossFX. [`adopt_preferred_language`] puts the
//! language Windows' own rule for apps picks into `LANG` for this process, first thing in `main`,
//! so that every reader of it agrees: the window, the notifications, the Settings pane's "System"
//! entry. That rule follows the user's *preferred languages* (Settings > Time & language >
//! Language & region), not the display language: an English Windows whose list starts with
//! Russian shows its apps in Russian. So the first of that list CrossFX has a table for wins;
//! failing that, the first display language with a table; failing that, English. Unless one of
//! the three POSIX variables is set already, which is somebody's word: the user's, or a
//! terminal's — Git for Windows' sets `LANG` from the display language.

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_MULTI_SZ, RegGetValueW};
use windows::core::{PCWSTR, PWSTR};

/// The user's preferred languages, most preferred first, as Windows keeps them for apps: the
/// list `GlobalizationPreferences.Languages` returns, which is the multi-string `Languages`
/// under `HKCU\Control Panel\International\User Profile` — `ru`, `en-US`, `zh-Hant-TW`. Empty
/// when there is none (a profile that never set one up).
#[must_use]
pub fn preferred_languages() -> Vec<String> {
    let key = super::wide(r"Control Panel\International\User Profile");
    let name = super::wide("Languages");
    let mut bytes = 0_u32;
    // SAFETY: NUL-terminated strings and an out-pointer to a local; a size query, no data.
    let sized = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_MULTI_SZ,
            None,
            None,
            Some(&raw mut bytes),
        )
    };
    if sized != ERROR_SUCCESS || bytes == 0 {
        return Vec::new();
    }
    // Two units more than asked for: room for the terminators `RegGetValueW` adds to a value
    // stored without them.
    let mut buffer = vec![0_u16; bytes as usize / 2 + 2];
    let mut size = u32::try_from(buffer.len() * 2).unwrap_or(bytes);
    // SAFETY: as above, with a buffer of `size` bytes that outlives the call.
    let read = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_MULTI_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&raw mut size),
        )
    };
    if read != ERROR_SUCCESS {
        return Vec::new();
    }
    buffer.truncate(size as usize / 2);
    multi_string(&buffer)
}

/// The user's display languages, most preferred first, as Windows names them: `ru-RU`,
/// `zh-Hant-TW`. Empty when Windows does not say.
#[must_use]
pub fn preferred_ui_languages() -> Vec<String> {
    let (mut count, mut units) = (0_u32, 0_u32);
    // SAFETY: a size query: out-pointers to locals, and no buffer.
    let sized = unsafe {
        GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &raw mut count, None, &raw mut units)
    };
    if sized.is_err() || units == 0 {
        return Vec::new();
    }
    let mut buffer = vec![0_u16; units as usize];
    // SAFETY: `buffer` holds the `units` the call was told it has, and outlives the call.
    let read = unsafe {
        GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &raw mut count,
            Some(PWSTR(buffer.as_mut_ptr())),
            &raw mut units,
        )
    };
    if read.is_err() {
        return Vec::new();
    }
    multi_string(&buffer)
}

/// The strings of a Windows multi-string: each one NUL-terminated, the list ended by an empty one.
fn multi_string(buffer: &[u16]) -> Vec<String> {
    buffer
        .split(|&unit| unit == 0)
        .take_while(|name| !name.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

/// What `LANG` says for the languages `preferred`: the first CrossFX has a table for, spelt the
/// POSIX way (`ru_RU`), which every reader of `LANG` takes. `None` for none of them.
fn lang_for(preferred: &[String]) -> Option<String> {
    preferred
        .iter()
        .find(|name| crossfx_core::i18n::language_for_locale(name).is_some())
        .map(|name| name.replace('-', "_"))
}

/// Windows' rule for apps over the user's `preferred` languages and their `display` languages:
/// the first preferred one with a table, else the first display one with a table. `None` —
/// English — for none.
fn choose(preferred: &[String], display: &[String]) -> Option<String> {
    lang_for(preferred).or_else(|| lang_for(display))
}

/// See the module docs. Nothing changes when one of the POSIX variables is set, or when none of
/// the languages has a table — English then, as on Linux.
pub fn adopt_preferred_language() {
    // The POSIX variables the session's language is read from — a list in place, not a `const`
    // table, which the translation audit would take for interface text.
    let posix = ["LC_ALL", "LC_MESSAGES", "LANG"];
    if posix
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
    {
        return;
    }
    if let Some(lang) = choose(&preferred_languages(), &preferred_ui_languages()) {
        // SAFETY: Windows synchronises a process's environment, which makes `set_var` sound from
        // any thread there (the standard library's own note); and this is the first thing `main`
        // does besides.
        unsafe { std::env::set_var("LANG", lang) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossfx_core::i18n::language_for_locale;

    fn wide(names: &str) -> Vec<u16> {
        names.encode_utf16().collect()
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|&name| name.to_owned()).collect()
    }

    #[test]
    fn a_multi_string_is_read_up_to_the_empty_one_that_ends_it() {
        assert_eq!(multi_string(&wide("ru-RU\0en-US\0\0")), ["ru-RU", "en-US"]);
        assert_eq!(multi_string(&wide("ru\0en-US")), ["ru", "en-US"]);
        assert_eq!(multi_string(&wide("\0\0")), Vec::<String>::new());
        assert_eq!(multi_string(&[]), Vec::<String>::new());
    }

    #[test]
    fn the_first_language_with_a_table_is_the_one_and_every_reader_takes_its_spelling() {
        assert_eq!(
            lang_for(&names(&["ru-RU", "en-US"])),
            Some("ru_RU".to_owned())
        );
        // Hebrew has no table: the next one does.
        assert_eq!(
            lang_for(&names(&["he-IL", "uk-UA"])),
            Some("uk_UA".to_owned())
        );
        assert_eq!(lang_for(&names(&["he-IL"])), None);
        assert_eq!(lang_for(&[]), None);
        for (windows, table) in [
            ("ru-RU", "ru"),
            ("uk-UA", "ua"),
            ("pt-BR", "pt-br"),
            ("zh-Hant-TW", "zh-TW"),
            ("zh-Hans-CN", "zh-CN"),
            ("nb-NO", "no"),
        ] {
            let lang = lang_for(&names(&[windows])).expect("a table");
            assert_eq!(
                language_for_locale(&lang),
                Some(table),
                "{windows} as {lang}"
            );
        }
    }

    #[test]
    fn the_preferred_languages_win_over_the_display_language() {
        let table = |preferred: &[&str], display: &[&str]| {
            choose(&names(preferred), &names(display))
                .as_deref()
                .and_then(language_for_locale)
        };
        // This machine's case: an English display language, a list that starts with Russian.
        assert_eq!(table(&["ru", "en-US"], &["en-US"]), Some("ru"));
        assert_eq!(table(&["en-US", "ru"], &["ru-RU"]), Some("en"));
        // A preferred language with no table gives way to the next preferred one…
        assert_eq!(table(&["he-IL", "de-DE"], &["fr-FR"]), Some("de"));
        // …and none with a table, or no list at all, to the display languages.
        assert_eq!(table(&["he-IL"], &["fr-FR"]), Some("fr"));
        assert_eq!(table(&[], &["zh-Hant-TW"]), Some("zh-TW"));
        // Nothing with a table anywhere: English, by leaving `LANG` alone.
        assert_eq!(choose(&names(&["he-IL"]), &names(&["yi"])), None);
        assert_eq!(choose(&[], &[]), None);
        // Bare codes and script forms, the way the preferred list spells them.
        for (windows, code) in [
            ("ru", "ru"),
            ("uk", "ua"),
            ("pt", "pt"),
            ("pt-BR", "pt-br"),
            ("zh-Hant", "zh-TW"),
            ("zh-Hant-TW", "zh-TW"),
            ("zh-Hant-HK", "zh-TW"),
            ("zh-Hans-CN", "zh-CN"),
            ("zh-CN", "zh-CN"),
            ("nb", "no"),
            ("bs-Latn-BA", "ba"),
        ] {
            assert_eq!(table(&[windows], &[]), Some(code), "{windows}");
        }
    }

    #[test]
    fn windows_names_the_users_preferred_languages() {
        let languages = preferred_languages();
        assert!(!languages.is_empty());
        assert!(
            languages
                .iter()
                .all(|name| name.len() >= 2 && !name.contains('\0')),
            "{languages:?}"
        );
        // Whatever this machine's list holds, the choice over it is the one the app makes.
        assert_eq!(
            choose(&languages, &[]).is_some(),
            languages
                .iter()
                .any(|name| language_for_locale(name).is_some())
        );
    }

    #[test]
    fn windows_names_the_users_display_languages() {
        let languages = preferred_ui_languages();
        assert!(!languages.is_empty());
        assert!(
            languages
                .iter()
                .all(|name| name.len() >= 2 && !name.contains('\0')),
            "{languages:?}"
        );
    }
}
