//! The languages each speech engine transcribes (#288), with their native
//! names — what the browser extension's side panel offers for a meeting
//! (`GET /app/languages`) and what the `/live` `start {language}` is
//! checked against. Pure — unit tested.
//!
//! - **Whisper**: the 99 languages of the multilingual models (whisper.cpp's
//!   codes: ISO 639-1, plus `haw` and whisper's own `jw` for Javanese).
//!   Cantonese (`yue`) is left out: only large-v3 knows it. An English-only
//!   model (`*.en.bin`) offers English alone.
//! - **Parakeet** TDT v3: its 25 European languages. It detects the
//!   language itself; the choice still sets the item's language and the
//!   cleanup's fillers (#218).
//! - **Qwen3-ASR**: its 30 languages (the 22 Chinese dialects and
//!   Cantonese come under Chinese), as 29 codes — Filipino is `tl`, as
//!   whisper names it. It detects the language itself too.
//!
//! `auto` (detect) is always accepted and never listed.

use crate::settings::{Settings, SttEngine};
use serde::Serialize;

/// Every code any engine offers, with its native name. Whisper's order.
const NAMES: &[(&str, &str)] = &[
    ("en", "English"),
    ("zh", "中文"),
    ("de", "Deutsch"),
    ("es", "Español"),
    ("ru", "Русский"),
    ("ko", "한국어"),
    ("fr", "Français"),
    ("ja", "日本語"),
    ("pt", "Português"),
    ("tr", "Türkçe"),
    ("pl", "Polski"),
    ("ca", "Català"),
    ("nl", "Nederlands"),
    ("ar", "العربية"),
    ("sv", "Svenska"),
    ("it", "Italiano"),
    ("id", "Bahasa Indonesia"),
    ("hi", "हिन्दी"),
    ("fi", "Suomi"),
    ("vi", "Tiếng Việt"),
    ("he", "עברית"),
    ("uk", "Українська"),
    ("el", "Ελληνικά"),
    ("ms", "Bahasa Melayu"),
    ("cs", "Čeština"),
    ("ro", "Română"),
    ("da", "Dansk"),
    ("hu", "Magyar"),
    ("ta", "தமிழ்"),
    ("no", "Norsk"),
    ("th", "ไทย"),
    ("ur", "اردو"),
    ("hr", "Hrvatski"),
    ("bg", "Български"),
    ("lt", "Lietuvių"),
    ("la", "Latina"),
    ("mi", "Māori"),
    ("ml", "മലയാളം"),
    ("cy", "Cymraeg"),
    ("sk", "Slovenčina"),
    ("te", "తెలుగు"),
    ("fa", "فارسی"),
    ("lv", "Latviešu"),
    ("bn", "বাংলা"),
    ("sr", "Српски"),
    ("az", "Azərbaycanca"),
    ("sl", "Slovenščina"),
    ("kn", "ಕನ್ನಡ"),
    ("et", "Eesti"),
    ("mk", "Македонски"),
    ("br", "Brezhoneg"),
    ("eu", "Euskara"),
    ("is", "Íslenska"),
    ("hy", "Հայերեն"),
    ("ne", "नेपाली"),
    ("mn", "Монгол"),
    ("bs", "Bosanski"),
    ("kk", "Қазақ тілі"),
    ("sq", "Shqip"),
    ("sw", "Kiswahili"),
    ("gl", "Galego"),
    ("mr", "मराठी"),
    ("pa", "ਪੰਜਾਬੀ"),
    ("si", "සිංහල"),
    ("km", "ខ្មែរ"),
    ("sn", "chiShona"),
    ("yo", "Yorùbá"),
    ("so", "Soomaali"),
    ("af", "Afrikaans"),
    ("oc", "Occitan"),
    ("ka", "ქართული"),
    ("be", "Беларуская"),
    ("tg", "Тоҷикӣ"),
    ("sd", "سنڌي"),
    ("gu", "ગુજરાતી"),
    ("am", "አማርኛ"),
    ("yi", "ייִדיש"),
    ("lo", "ລາວ"),
    ("uz", "Oʻzbekcha"),
    ("fo", "Føroyskt"),
    ("ht", "Kreyòl ayisyen"),
    ("ps", "پښتو"),
    ("tk", "Türkmençe"),
    ("nn", "Nynorsk"),
    ("mt", "Malti"),
    ("sa", "संस्कृतम्"),
    ("lb", "Lëtzebuergesch"),
    ("my", "မြန်မာ"),
    ("bo", "བོད་སྐད་"),
    ("tl", "Tagalog"),
    ("mg", "Malagasy"),
    ("as", "অসমীয়া"),
    ("tt", "Татарча"),
    ("haw", "ʻŌlelo Hawaiʻi"),
    ("ln", "Lingála"),
    ("ha", "Hausa"),
    ("ba", "Башҡортса"),
    ("jw", "Basa Jawa"),
    ("su", "Basa Sunda"),
];

/// Parakeet TDT 0.6B v3's languages.
const PARAKEET: &[&str] = &[
    "bg", "hr", "cs", "da", "nl", "en", "et", "fi", "fr", "de", "el", "hu", "it", "lv", "lt", "mt",
    "pl", "pt", "ro", "sk", "sl", "es", "sv", "ru", "uk",
];

/// Qwen3-ASR's languages (Cantonese and the Chinese dialects under `zh`).
const QWEN3_ASR: &[&str] = &[
    "zh", "en", "ar", "de", "fr", "es", "pt", "id", "it", "ko", "ru", "th", "vi", "ja", "tr", "hi",
    "ms", "nl", "sv", "da", "fi", "pl", "cs", "tl", "fa", "el", "hu", "mk", "ro",
];

/// Detect the language (always accepted, never listed).
pub const AUTO: &str = "auto";

/// A language an engine offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Language {
    pub code: &'static str,
    /// In the language itself: "Italiano", "日本語".
    pub name: &'static str,
}

/// Which list of languages applies: follows the dictation's engine (and,
/// for Whisper, whether the model is English-only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LanguageSet {
    #[default]
    Whisper,
    WhisperEnglish,
    Parakeet,
    Qwen3Asr,
}

impl LanguageSet {
    pub fn for_settings(settings: &Settings) -> Self {
        match settings.engine {
            SttEngine::Whisper if is_english_only(&settings.whisper_model) => Self::WhisperEnglish,
            SttEngine::Whisper => Self::Whisper,
            SttEngine::Parakeet => Self::Parakeet,
            SttEngine::Qwen3Asr => Self::Qwen3Asr,
        }
    }

    /// The engine as the settings name it (`whisper`, `parakeet`,
    /// `qwen3_asr`).
    pub fn engine(self) -> &'static str {
        match self {
            Self::Whisper | Self::WhisperEnglish => "whisper",
            Self::Parakeet => "parakeet",
            Self::Qwen3Asr => "qwen3_asr",
        }
    }

    /// For messages: "Whisper", "Parakeet", "Qwen3-ASR".
    pub fn engine_label(self) -> &'static str {
        match self {
            Self::Whisper => "Whisper",
            Self::WhisperEnglish => "This English-only Whisper model",
            Self::Parakeet => "Parakeet",
            Self::Qwen3Asr => "Qwen3-ASR",
        }
    }

    /// The codes, in the engine's order.
    pub fn codes(self) -> Vec<&'static str> {
        match self {
            Self::Whisper => NAMES.iter().map(|(c, _)| *c).collect(),
            Self::WhisperEnglish => vec!["en"],
            Self::Parakeet => PARAKEET.to_vec(),
            Self::Qwen3Asr => QWEN3_ASR.to_vec(),
        }
    }

    /// The languages with their native names.
    pub fn languages(self) -> Vec<Language> {
        self.codes()
            .into_iter()
            .filter_map(|code| native_name(code).map(|name| Language { code, name }))
            .collect()
    }

    pub fn supports(self, code: &str) -> bool {
        self.codes().contains(&code)
    }
}

/// `ggml-base.en.bin`, `small.en`: an English-only whisper model.
fn is_english_only(model: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    let m = m.strip_suffix(".bin").unwrap_or(&m);
    m.ends_with(".en")
}

/// The native name of a code any engine offers.
pub fn native_name(code: &str) -> Option<&'static str> {
    NAMES.iter().find(|(c, _)| *c == code).map(|(_, n)| *n)
}

/// A language code as sent: trimmed, lowercased, `en-US` / `en_US` → `en`.
/// `None` unless the primary part is 2–3 ASCII letters (or `auto`).
pub fn normalize(code: &str) -> Option<String> {
    let code = code.trim().to_ascii_lowercase();
    if code == AUTO {
        return Some(code);
    }
    let primary = code.split(['-', '_']).next().unwrap_or_default();
    let ok = (2..=3).contains(&primary.len()) && primary.chars().all(|c| c.is_ascii_lowercase());
    ok.then(|| primary.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_lists_named_unique_languages() {
        assert_eq!(LanguageSet::Whisper.codes().len(), 99);
        assert_eq!(LanguageSet::Parakeet.codes().len(), 25);
        assert_eq!(LanguageSet::Qwen3Asr.codes().len(), 29);
        for set in [
            LanguageSet::Whisper,
            LanguageSet::WhisperEnglish,
            LanguageSet::Parakeet,
            LanguageSet::Qwen3Asr,
        ] {
            let codes = set.codes();
            let langs = set.languages();
            assert_eq!(
                langs.len(),
                codes.len(),
                "{set:?}: every code has a native name"
            );
            let mut unique = codes.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(unique.len(), codes.len(), "{set:?}: no duplicates");
            assert!(!set.supports(AUTO), "auto is never listed");
        }
        assert_eq!(native_name("it"), Some("Italiano"));
        assert_eq!(native_name("en"), Some("English"));
        assert_eq!(native_name("xx"), None);
    }

    #[test]
    fn the_set_follows_the_engine_and_model() {
        let mut s = Settings {
            engine: SttEngine::Whisper,
            whisper_model: "ggml-large-v3-turbo-q5_0.bin".into(),
            ..Default::default()
        };
        assert_eq!(LanguageSet::for_settings(&s), LanguageSet::Whisper);
        s.whisper_model = "ggml-base.en.bin".into();
        assert_eq!(LanguageSet::for_settings(&s), LanguageSet::WhisperEnglish);
        assert_eq!(LanguageSet::WhisperEnglish.codes(), ["en"]);
        s.engine = SttEngine::Parakeet;
        assert_eq!(LanguageSet::for_settings(&s), LanguageSet::Parakeet);
        assert!(LanguageSet::Parakeet.supports("it") && !LanguageSet::Parakeet.supports("ja"));
        s.engine = SttEngine::Qwen3Asr;
        assert_eq!(LanguageSet::for_settings(&s), LanguageSet::Qwen3Asr);
        assert_eq!(LanguageSet::Qwen3Asr.engine(), "qwen3_asr");
    }

    #[test]
    fn codes_are_normalized() {
        assert_eq!(normalize(" EN "), Some("en".into()));
        assert_eq!(normalize("en-US"), Some("en".into()));
        assert_eq!(normalize("pt_BR"), Some("pt".into()));
        assert_eq!(normalize("Auto"), Some("auto".into()));
        assert_eq!(normalize("haw"), Some("haw".into()));
        for bad in ["", "e", "engl", "e1", "../", "日本"] {
            assert_eq!(normalize(bad), None, "{bad}");
        }
    }
}
