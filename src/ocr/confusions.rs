use regex::Regex;
use std::sync::OnceLock;

fn re_contraction() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // l'd / l'm / l've / l'll (either apostrophe) -> I'd / I'm / I've / I'll.
    // The apostrophe is captured, not assumed, so the replacement preserves
    // whichever one Vision actually emitted.
    R.get_or_init(|| Regex::new(r"\bl(['\u{2019}])(d|m|ve|ll)\b").unwrap())
}

fn re_standalone() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // A lowercase "l" as its own word -> "I". `\b` alone can't tell a truly
    // standalone "l" apart from the "l" that opens a French elision like
    // "l'eau": an apostrophe is a non-word character, so `\b` sits in
    // exactly the same place before a space and before an apostrophe. The
    // trailing group makes that distinction explicit by requiring the next
    // character (if any) not be an apostrophe.
    R.get_or_init(|| Regex::new(r"\bl\b([^'\u{2019}]|$)").unwrap())
}

/// Fixes a small set of unambiguous OCR misreads where Apple's Vision (or
/// any OCR engine) returns lowercase `l` where the source text had
/// uppercase `I`. In most sans-serif fonts the two glyphs are both a bare
/// vertical stroke, so this is an inherent OCR ambiguity, not a defect in
/// our pipeline or a gap `usesLanguageCorrection` should be expected to
/// close — "l'd", "l'm", "l've", "l'll" and a bare "l" are not real words
/// in any dictionary a language-correction pass checks against, so it has
/// nothing to correct *to*.
///
/// Only two contexts are touched, both unambiguous:
/// - `l'd` / `l'm` / `l've` / `l'll` (either apostrophe character) ->
///   `I'd` / `I'm` / `I've` / `I'll`
/// - a standalone lowercase `l` as a whole word -> `I`
///
/// These four contractions and the standalone pronoun are the only shapes
/// touched because they are unambiguous in every language Aloud ships:
/// English, German, Ukrainian and Russian have no real word "l'd", "l'm",
/// "l've", "l'll", or standalone "l". **Do not widen this list.** French
/// elisions (`l'eau`, `l'autre`, `l'école`, ...) share the exact
/// "l" + apostrophe shape and must stay untouched — that is why the
/// contraction regex matches only these four specific suffixes, and the
/// standalone-word regex explicitly excludes any `l` immediately followed
/// by an apostrophe.
///
/// Applies ONLY to OCR output. Selected text (the Services flow) is the
/// user's own exact characters and must never be "corrected" — callers
/// must not run this over `speak_selection`'s input. See
/// `crate::app::actions::read_region`, which is the sole caller.
pub fn fix_confusions(input: &str) -> String {
    let s = re_contraction().replace_all(input, "I$1$2").into_owned();
    re_standalone().replace_all(&s, "I$1").into_owned()
}

#[cfg(test)]
mod tests {
    use super::fix_confusions;

    #[test]
    fn fixes_ld_contraction_with_ascii_apostrophe() {
        assert_eq!(
            fix_confusions("l'd love to hear from you."),
            "I'd love to hear from you."
        );
    }

    #[test]
    fn fixes_ld_contraction_with_typographic_apostrophe_and_preserves_it() {
        let result = fix_confusions("l\u{2019}d love to hear from you.");
        assert_eq!(result, "I\u{2019}d love to hear from you.");
        // The typographic apostrophe must survive, not get normalized to ASCII.
        assert!(result.contains('\u{2019}'));
    }

    #[test]
    fn fixes_standalone_lowercase_l() {
        assert_eq!(fix_confusions("l am here"), "I am here");
    }

    #[test]
    fn fixes_lm_lve_lll_contractions() {
        assert_eq!(fix_confusions("l'm ready"), "I'm ready");
        assert_eq!(fix_confusions("l've seen it"), "I've seen it");
        assert_eq!(fix_confusions("l'll go"), "I'll go");
    }

    #[test]
    fn does_not_touch_words_containing_l() {
        assert_eq!(fix_confusions("well"), "well");
        assert_eq!(fix_confusions("hello"), "hello");
        assert_eq!(fix_confusions("will"), "will");
    }

    #[test]
    fn does_not_touch_french_elision() {
        assert_eq!(fix_confusions("l'eau est froide"), "l'eau est froide");
    }

    #[test]
    fn does_not_touch_german_words_containing_l() {
        let s = "Ich lese jeden Tag ein Buch, weil es mir gefällt.";
        assert_eq!(fix_confusions(s), s);
    }

    #[test]
    fn is_idempotent() {
        let input = "l'd love to hear from you, l am here too.";
        let once = fix_confusions(input);
        let twice = fix_confusions(&once);
        assert_eq!(once, twice);
    }
}
