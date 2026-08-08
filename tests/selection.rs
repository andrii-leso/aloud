//! Tests for the one genuinely testable piece of the selection path.
//!
//! What is NOT tested here, and cannot be: that macOS actually delivers a
//! selection to our Service. That is pure system integration — it needs an
//! installed `.app` bundle, a Services menu, and a human selecting text —
//! and a mock of it would assert nothing about the real behaviour. It is
//! verified by hand in Task 6.
//!
//! What IS tested: `selection_worth_speaking`, the pure decision the
//! Objective-C callback makes about the string it read off the pasteboard.

use aloud::selection::selection_worth_speaking;

#[test]
fn ordinary_text_passes_through_unchanged() {
    assert_eq!(
        selection_worth_speaking(Some("Hello there")),
        Some("Hello there")
    );
}

#[test]
fn surrounding_whitespace_is_not_trimmed() {
    // Cleanup is the TextNormalizer's job downstream, not this function's.
    // If this ever starts trimming, the selection path and the OCR path
    // clean text in two different places.
    let raw = "  padded\n";
    assert_eq!(selection_worth_speaking(Some(raw)), Some(raw));
}

#[test]
fn internal_structure_is_preserved() {
    let raw = "First line.\n\nSecond paragraph — with an em dash and ünïcode.";
    assert_eq!(selection_worth_speaking(Some(raw)), Some(raw));
}

#[test]
fn no_string_on_the_pasteboard_is_nothing_to_say() {
    assert_eq!(selection_worth_speaking(None), None);
}

#[test]
fn empty_string_is_nothing_to_say() {
    assert_eq!(selection_worth_speaking(Some("")), None);
}

#[test]
fn whitespace_only_is_nothing_to_say() {
    // A stray selection of a blank line or an indent should not produce an
    // empty utterance.
    assert_eq!(selection_worth_speaking(Some("   \t\n \u{00a0}")), None);
}
