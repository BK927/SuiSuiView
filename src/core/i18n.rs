mod en_us;
mod ko_kr;

use en_us::CATALOG_EN_US;
use ko_kr::CATALOG_KO_KR;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::LazyLock;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    #[default]
    Auto,
    KoKr,
    EnUs,
}

impl Language {
    pub const ALL: [Self; 3] = [Self::Auto, Self::KoKr, Self::EnUs];

    pub fn resolve(self) -> ResolvedLanguage {
        match self {
            Self::Auto => system_language(),
            Self::KoKr => ResolvedLanguage::KoKr,
            Self::EnUs => ResolvedLanguage::EnUs,
        }
    }

    pub fn label(self, i18n: I18n) -> String {
        match self {
            Self::Auto => i18n.text("language.auto"),
            Self::KoKr => i18n.text("language.ko_kr"),
            Self::EnUs => i18n.text("language.en_us"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedLanguage {
    KoKr,
    EnUs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct I18n {
    language: ResolvedLanguage,
}

impl I18n {
    pub fn from_language(language: Language) -> Self {
        Self {
            language: language.resolve(),
        }
    }

    pub fn resolved(language: ResolvedLanguage) -> Self {
        Self { language }
    }

    pub fn language(self) -> ResolvedLanguage {
        self.language
    }

    pub fn text(self, key: &str) -> String {
        catalog(self.language)
            .get(key)
            .or_else(|| catalog(ResolvedLanguage::EnUs).get(key))
            .map_or_else(|| key.to_owned(), |value| (*value).to_owned())
    }

    pub fn with_vars(self, key: &str, vars: &[(&str, String)]) -> String {
        let mut text = self.text(key);
        for (name, value) in vars {
            text = text.replace(&format!("{{{name}}}"), value);
        }
        text
    }

    pub fn experimental_label(self, name: &str) -> String {
        format!("{name} ({})", self.text("state.experimental"))
    }

    pub fn slow_label(self, name: &str) -> String {
        format!("{name} ({})", self.text("state.slow"))
    }
}

fn catalog(language: ResolvedLanguage) -> &'static BTreeMap<&'static str, &'static str> {
    match language {
        ResolvedLanguage::KoKr => &KO_KR_CATALOG,
        ResolvedLanguage::EnUs => &EN_US_CATALOG,
    }
}

static KO_KR_CATALOG: LazyLock<BTreeMap<&'static str, &'static str>> =
    LazyLock::new(|| CATALOG_KO_KR.iter().copied().collect());
static EN_US_CATALOG: LazyLock<BTreeMap<&'static str, &'static str>> =
    LazyLock::new(|| CATALOG_EN_US.iter().copied().collect());

fn system_language() -> ResolvedLanguage {
    // UI language changes take effect on the next application launch. Avoid
    // querying Win32 and allocating a UTF-16 buffer in every paint frame.
    static SYSTEM_LANGUAGE: LazyLock<ResolvedLanguage> = LazyLock::new(|| {
        system_locale_name()
            .filter(|locale| locale_is_korean(locale))
            .map_or(ResolvedLanguage::EnUs, |_| ResolvedLanguage::KoKr)
    });
    *SYSTEM_LANGUAGE
}

fn locale_is_korean(locale: &str) -> bool {
    let locale = locale.trim().to_ascii_lowercase();
    locale == "ko" || locale.starts_with("ko-") || locale.starts_with("ko_")
}

fn system_locale_name() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        if let Some(locale) = windows_locale_name() {
            return Some(locale);
        }
    }

    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.trim().is_empty())
}

#[cfg(target_os = "windows")]
fn windows_locale_name() -> Option<String> {
    use windows_sys::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};

    // Regional formatting can differ from the Windows display language.
    // The first preferred UI language is authoritative.
    let mut count = 0;
    let mut length = 0;
    if unsafe { GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, std::ptr::null_mut(), &mut length) } == 0
        || length == 0
    {
        return None;
    }
    let mut buffer = vec![0_u16; length as usize];
    if unsafe { GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, buffer.as_mut_ptr(), &mut length) } == 0 {
        return None;
    }
    let end = buffer.iter().position(|&value| value == 0)?;
    (end > 0).then(|| String::from_utf16_lossy(&buffer[..end]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogs_have_matching_keys() {
        let ko_keys = catalog(ResolvedLanguage::KoKr)
            .keys()
            .copied()
            .collect::<Vec<_>>();
        let en_keys = catalog(ResolvedLanguage::EnUs)
            .keys()
            .copied()
            .collect::<Vec<_>>();

        assert_eq!(ko_keys, en_keys);
    }

    #[test]
    fn catalogs_have_unique_nonempty_entries_and_matching_variables() {
        fn variables(text: &str) -> std::collections::BTreeSet<&str> {
            text.split('{').skip(1).filter_map(|tail| tail.split_once('}').map(|(name, _)| name)).collect()
        }
        for entries in [CATALOG_KO_KR, CATALOG_EN_US] {
            let mut keys = std::collections::BTreeSet::new();
            for &(key, value) in entries {
                assert!(keys.insert(key), "duplicate translation: {key}");
                assert!(!value.trim().is_empty(), "empty translation: {key}");
            }
        }
        for &(key, ko) in CATALOG_KO_KR {
            let en = catalog(ResolvedLanguage::EnUs)[key];
            assert_eq!(variables(ko), variables(en), "translation variables: {key}");
        }
    }

    #[test]
    fn language_selection_handles_korean_locales_and_manual_overrides() {
        for locale in ["ko", "ko-KR", "KO_kr", " ko-KP "] { assert!(locale_is_korean(locale)); }
        for locale in ["en-US", "ja-JP", "", "kok-IN"] { assert!(!locale_is_korean(locale)); }
        assert_eq!(Language::KoKr.resolve(), ResolvedLanguage::KoKr);
        assert_eq!(Language::EnUs.resolve(), ResolvedLanguage::EnUs);
    }

    #[test]
    fn localized_state_words_are_translated() {
        let ko = I18n::resolved(ResolvedLanguage::KoKr);
        let en = I18n::resolved(ResolvedLanguage::EnUs);

        assert_eq!(ko.text("state.default"), "기본값");
        assert_eq!(en.text("state.default"), "Default");
        assert_eq!(ko.experimental_label("ArtCNN C4F16"), "ArtCNN C4F16 (실험)");
        assert_eq!(ko.slow_label("SR Lab SPAN x2"), "SR Lab SPAN x2 (느림)");
        assert_eq!(en.slow_label("SR Lab SPAN x2"), "SR Lab SPAN x2 (Slow)");
        assert_eq!(
            en.experimental_label("ArtCNN C4F16"),
            "ArtCNN C4F16 (Experimental)"
        );
    }
}
