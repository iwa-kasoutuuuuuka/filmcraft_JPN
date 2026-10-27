//! The language the user runs the operating system in, for the UI language setting "Auto".
//!
//! Windows reports its display language (`GetUserDefaultUILanguage`). Elsewhere the POSIX locale
//! variables are read (`LC_ALL`, `LC_MESSAGES`, `LANG`); a macOS app started from the Finder has
//! none of them set, so there the answer is `None` and the caller uses its default.

/// The user's display language as a BCP 47 tag (`ja-JP`, `en-US`), or None when the system does
/// not say.
pub fn system_language() -> Option<String> {
    #[cfg(windows)]
    {
        win::ui_language()
    }
    #[cfg(not(windows))]
    {
        ["LC_ALL", "LC_MESSAGES", "LANG"].into_iter().filter_map(|k| std::env::var(k).ok()).find_map(|v| posix_locale_tag(&v))
    }
}

/// `ja_JP.UTF-8@modifier` → `ja-JP`. The `C` / `POSIX` locales and empty values name no language.
pub fn posix_locale_tag(value: &str) -> Option<String> {
    let v = value.split(['.', '@']).next().unwrap_or_default().trim();
    if v.is_empty() || v.eq_ignore_ascii_case("C") || v.eq_ignore_ascii_case("POSIX") {
        return None;
    }
    v.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-').then(|| v.replace('_', "-"))
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod win {
    use windows::Win32::Globalization::{GetUserDefaultUILanguage, LCIDToLocaleName};

    /// `LOCALE_NAME_MAX_LENGTH`: the longest locale name, terminator included.
    const NAME_MAX: usize = 85;

    pub fn ui_language() -> Option<String> {
        // SAFETY: takes no arguments and only reads the calling user's settings.
        let lang = unsafe { GetUserDefaultUILanguage() };
        let mut name = [0u16; NAME_MAX];
        // SAFETY: `name` is a writable buffer whose length the slice carries; the function writes
        // at most that many UTF-16 units and returns how many, terminator included (0 on failure).
        let n = unsafe { LCIDToLocaleName(u32::from(lang), Some(&mut name), 0) };
        let len = usize::try_from(n).ok()?.checked_sub(1)?;
        String::from_utf16(name.get(..len)?).ok().filter(|s| !s.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_locales() {
        assert_eq!(posix_locale_tag("ja_JP.UTF-8").as_deref(), Some("ja-JP"));
        assert_eq!(posix_locale_tag("en_US").as_deref(), Some("en-US"));
        assert_eq!(posix_locale_tag("sr_RS.UTF-8@latin").as_deref(), Some("sr-RS"));
        assert_eq!(posix_locale_tag("ja").as_deref(), Some("ja"));
        for none in ["", "C", "POSIX", "C.UTF-8", "  ", "ja JP", "ja/../x"] {
            assert_eq!(posix_locale_tag(none), None, "{none:?}");
        }
    }

    /// Whatever the machine says is a plain language tag (or nothing): never a panic.
    #[test]
    fn system_language_is_a_tag() {
        let l = system_language();
        eprintln!("system language: {l:?}");
        if let Some(l) = l {
            assert!(!l.is_empty() && l.len() < 86 && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'), "{l}");
        }
        if cfg!(windows) {
            assert!(system_language().is_some(), "Windows always has a display language");
        }
    }
}
