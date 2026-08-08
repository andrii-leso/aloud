use lingua::{Language, LanguageDetector, LanguageDetectorBuilder};
use std::sync::OnceLock;

/// Minimum confidence before we trust a detection. Below this we send "na",
/// Supertonic's own unknown-language fallback.
const MIN_CONFIDENCE: f64 = 0.55;

/// Too little text to judge; short strings produce wild guesses.
const MIN_CHARS: usize = 12;

fn detector() -> &'static LanguageDetector {
    static D: OnceLock<LanguageDetector> = OnceLock::new();
    // Explicit subset. Never `from_all_languages()` — the full model data
    // is enormous and would bloat the binary.
    D.get_or_init(|| {
        LanguageDetectorBuilder::from_languages(&[
            Language::English,
            Language::German,
            Language::Ukrainian,
            Language::Russian,
        ])
        .build()
    })
}

fn to_supertonic_code(lang: Language) -> &'static str {
    match lang {
        Language::English => "en",
        Language::German => "de",
        Language::Ukrainian => "uk",
        Language::Russian => "ru",
        _ => "na",
    }
}

/// Best-effort language tag for Supertonic. Returns "na" when unsure.
pub fn detect_lang(text: &str) -> String {
    if text.trim().chars().count() < MIN_CHARS {
        return "na".to_string();
    }
    // lingua 1.8's compute_language_confidence_values returns Vec<(Language, f64)>
    // sorted descending by confidence, not a struct with .language()/.value().
    let values = detector().compute_language_confidence_values(text);
    match values.first() {
        Some((lang, confidence)) if *confidence >= MIN_CONFIDENCE => {
            to_supertonic_code(*lang).to_string()
        }
        _ => "na".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::detect_lang;

    #[test]
    fn detects_english() {
        assert_eq!(
            detect_lang("The applicant must submit the completed form within four weeks."),
            "en"
        );
    }

    #[test]
    fn detects_german() {
        assert_eq!(
            detect_lang("Der Antragsteller muss das ausgefüllte Formular innerhalb von vier Wochen einreichen."),
            "de"
        );
    }

    #[test]
    fn detects_ukrainian() {
        assert_eq!(
            detect_lang("Заявники мають подати документи до п'ятниці, інакше їхні справи буде закрито."),
            "uk"
        );
    }

    #[test]
    fn falls_back_to_na_on_junk() {
        assert_eq!(detect_lang("asdf qwer zxcv 12345 ..."), "na");
    }

    #[test]
    fn falls_back_to_na_on_empty() {
        assert_eq!(detect_lang(""), "na");
    }
}
