//! The language the Windows desktop is shown in, for the interface's "the system's language".
//!
//! `fxsound_core::i18n::system_language` reads the session's language the POSIX way — `LC_ALL`,
//! then `LC_MESSAGES`, then `LANG` — and so does `fxsound_audio`'s locale. Windows sets none of
//! them, so a Russian Windows got an English FxSound. [`adopt_display_language`] puts the user's
//! display language — the first of their preferred ones FxSound has a table for — into `LANG`
//! for this process, first thing in `main`, so that every reader of it agrees: the window, the
//! notifications, the Settings pane's "System" entry. Unless one of the three is set already,
//! which is somebody's word: the user's, or a terminal's — Git for Windows' sets `LANG` from this
//! same display language.

use windows::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
use windows::core::PWSTR;

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

/// What `LANG` says for the display languages `preferred`: the first FxSound has a table for,
/// spelt the POSIX way (`ru_RU`), which every reader of `LANG` takes. `None` for none of them.
fn lang_for(preferred: &[String]) -> Option<String> {
    preferred
        .iter()
        .find(|name| fxsound_core::i18n::language_for_locale(name).is_some())
        .map(|name| name.replace('-', "_"))
}

/// See the module docs. Nothing changes when one of the POSIX variables is set, or when none of
/// the display languages has a table — English then, as on Linux.
pub fn adopt_display_language() {
    // The POSIX variables the session's language is read from — a list in place, not a `const`
    // table, which the translation audit would take for interface text.
    let posix = ["LC_ALL", "LC_MESSAGES", "LANG"];
    if posix
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
    {
        return;
    }
    if let Some(lang) = lang_for(&preferred_ui_languages()) {
        // SAFETY: Windows synchronises a process's environment, which makes `set_var` sound from
        // any thread there (the standard library's own note); and this is the first thing `main`
        // does besides.
        unsafe { std::env::set_var("LANG", lang) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxsound_core::i18n::language_for_locale;

    fn wide(names: &str) -> Vec<u16> {
        names.encode_utf16().collect()
    }

    #[test]
    fn a_multi_string_is_read_up_to_the_empty_one_that_ends_it() {
        assert_eq!(multi_string(&wide("ru-RU\0en-US\0\0")), ["ru-RU", "en-US"]);
        assert_eq!(multi_string(&wide("\0\0")), Vec::<String>::new());
        assert_eq!(multi_string(&[]), Vec::<String>::new());
    }

    #[test]
    fn the_first_display_language_with_a_table_is_the_one_and_every_reader_takes_its_spelling() {
        let names = |list: &[&str]| list.iter().map(|&name| name.to_owned()).collect::<Vec<_>>();
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
