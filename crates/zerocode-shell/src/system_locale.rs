//! The operating system language translated into the small catalog we ship.
//!
//! Asking the platform and deciding which catalog to use are deliberately
//! separate. Native APIs return an ordered list, while matching that list is
//! deterministic product policy and can be tested without changing a runner's
//! language settings.

/// The choices shown by the settings picker.
///
/// `system` is an instruction rather than a language and is therefore never
/// returned by [`system_locale`]. Korean is the source catalog and the safe
/// fallback when none of the preferred languages is available.
pub(crate) const LOCALES: &[&str] = &["system", "ko", "en", "ja", "zh", "es"];

const SOURCE_LOCALE: &str = "ko";

/// Resolve the platform's ordered language tags to the first catalog we ship.
///
/// Native Apple and Windows APIs return BCP-47 tags. Unix may also return a
/// POSIX spelling such as `en_US.UTF-8`, so only the primary language subtag
/// before a territory, encoding, or modifier is compared.
pub(crate) fn resolve_supported_locale<I, S>(preferred: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    preferred
        .into_iter()
        .find_map(|locale| supported_primary_subtag(locale.as_ref()))
        .unwrap_or(SOURCE_LOCALE)
        .to_owned()
}

/// Read every preferred UI language in the order supplied by the OS.
pub(crate) fn system_locale() -> String {
    resolve_supported_locale(sys_locale::get_locales())
}

fn supported_primary_subtag(locale: &str) -> Option<&'static str> {
    let primary = locale
        .trim()
        .split(['.', '@'])
        .next()
        .unwrap_or_default()
        .split(['-', '_'])
        .next()
        .unwrap_or_default();

    LOCALES
        .iter()
        .copied()
        .skip(1)
        .find(|supported| primary.eq_ignore_ascii_case(supported))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_region_or_script_still_chooses_its_language() {
        assert_eq!(resolve_supported_locale(["en-US"]), "en");
        assert_eq!(resolve_supported_locale(["zh-Hans-CN"]), "zh");
        assert_eq!(resolve_supported_locale(["JA_jp.UTF-8"]), "ja");
    }

    #[test]
    fn the_first_supported_preference_wins() {
        assert_eq!(resolve_supported_locale(["de-DE", "ja-JP", "en-US"]), "ja");
    }

    #[test]
    fn an_unknown_or_posix_locale_uses_the_source_catalog() {
        assert_eq!(resolve_supported_locale(["de-DE"]), SOURCE_LOCALE);
        assert_eq!(resolve_supported_locale(["C.UTF-8"]), SOURCE_LOCALE);
        assert_eq!(resolve_supported_locale(["POSIX"]), SOURCE_LOCALE);
        assert_eq!(
            resolve_supported_locale(std::iter::empty::<&str>()),
            SOURCE_LOCALE
        );
    }

    #[test]
    fn system_is_never_treated_as_a_language() {
        assert_eq!(resolve_supported_locale(["system"]), SOURCE_LOCALE);
    }
}
