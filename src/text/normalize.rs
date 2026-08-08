use regex::Regex;
use std::sync::OnceLock;

fn re_hyphen_break() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // word- \n word  ->  wordword   (single newline only)
    R.get_or_init(|| Regex::new(r"(\w)-\n(?:[ \t]*)(\w)").unwrap())
}

fn re_soft_break() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // A single newline not adjacent to another newline.
    R.get_or_init(|| Regex::new(r"(?m)([^\n])\n([^\n])").unwrap())
}

fn re_spaces() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"[ \t]+").unwrap())
}

/// A block of text is a page number if, once trimmed, it is nothing but
/// 1-4 digits. Used to drop page-number blocks wherever they land: start,
/// middle, or end of the document.
fn is_page_number_block(block: &str) -> bool {
    let t = block.trim();
    !t.is_empty() && t.len() <= 4 && t.chars().all(|c| c.is_ascii_digit())
}

/// Cleans OCR layout artifacts so the text reads as prose.
///
/// Order matters:
/// 1. Normalise line endings.
/// 2. Trim every line individually, *before* any structural pass runs —
///    OCR routinely leaves trailing spaces on otherwise-blank lines
///    (e.g. `"\n \n"`), and every later pass depends on a blank line
///    being pristine `"\n\n"`.
/// 3. Drop page-number blocks on the now-pristine `"\n\n"` boundaries.
///    Splitting the whole document into blocks (rather than an anchored
///    regex requiring `\n\n` on both sides) is what catches a page number
///    at the very start or end of the text, where one side has no
///    neighbouring blank line at all.
/// 4. Hyphen rejoin, before soft breaks would obscure the split word.
/// 5. Soft-break join to a fixpoint.
/// 6. Collapse whitespace runs and trim the result.
pub fn normalize_ocr(input: &str) -> String {
    let s = input.replace("\r\n", "\n");

    let s: String = s.split('\n').map(str::trim).collect::<Vec<_>>().join("\n");

    let s: String = s
        .split("\n\n")
        .filter(|block| !is_page_number_block(block))
        .collect::<Vec<_>>()
        .join("\n\n");

    let mut s = re_hyphen_break().replace_all(&s, "$1$2").into_owned();

    // Join soft line breaks. Run repeatedly to handle single-character lines
    // (overlapping matches that a single replace_all would miss).
    while re_soft_break().is_match(&s) {
        s = re_soft_break().replace_all(&s, "$1 $2").into_owned();
    }

    s = re_spaces().replace_all(&s, " ").into_owned();
    s.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::normalize_ocr;

    #[test]
    fn rejoins_hyphenated_line_breaks() {
        let input = "The applicant must submit the informa-\ntion before the dead-\nline.";
        assert_eq!(
            normalize_ocr(input),
            "The applicant must submit the information before the deadline."
        );
    }

    #[test]
    fn joins_soft_line_breaks_within_a_paragraph() {
        let input = "The applicant must submit\nthe completed form\nwithin four weeks.";
        assert_eq!(
            normalize_ocr(input),
            "The applicant must submit the completed form within four weeks."
        );
    }

    #[test]
    fn preserves_paragraph_breaks() {
        let input = "First paragraph here.\n\nSecond paragraph here.";
        assert_eq!(
            normalize_ocr(input),
            "First paragraph here.\n\nSecond paragraph here."
        );
    }

    #[test]
    fn strips_standalone_page_numbers() {
        let input = "End of the section.\n\n7\n\nStart of the next one.";
        assert_eq!(
            normalize_ocr(input),
            "End of the section.\n\nStart of the next one."
        );
    }

    #[test]
    fn collapses_runs_of_whitespace() {
        assert_eq!(normalize_ocr("too    many\t\tspaces"), "too many spaces");
    }

    #[test]
    fn does_not_join_a_hyphen_that_ends_a_sentence() {
        // A hyphen followed by a blank line is a paragraph boundary, not a split word.
        let input = "a well-known case-\n\nNext paragraph.";
        assert_eq!(
            normalize_ocr(input),
            "a well-known case-\n\nNext paragraph."
        );
    }

    #[test]
    fn empty_input_is_empty_output() {
        assert_eq!(normalize_ocr(""), "");
    }

    #[test]
    fn handles_single_character_lines_in_soft_breaks() {
        // Regression: single-char lines cause overlapping matches that a single replace_all misses
        assert_eq!(normalize_ocr("a\nb\nc"), "a b c");
    }

    #[test]
    fn handles_multiple_single_character_lines() {
        // Regression: multiple consecutive single-char lines all need joining
        assert_eq!(normalize_ocr("x\ny\nz\nw"), "x y z w");
    }

    #[test]
    fn preserves_digit_in_middle_of_text() {
        // Regression: a digit mid-line is NOT a page number (no surrounding blank lines)
        // Only standalone page numbers surrounded by blank lines get stripped.
        assert_eq!(normalize_ocr("Total:\n7\nEUR\n42"), "Total: 7 EUR 42");
    }

    #[test]
    fn strips_consecutive_page_numbers() {
        // Regression: consecutive page markers (running footers) need multiple passes
        assert_eq!(normalize_ocr("End.\n\n7\n\n8\n\nStart."), "End.\n\nStart.");
    }

    #[test]
    fn trims_whitespace_on_blank_lines_before_structural_passes() {
        // Regression: OCR routinely emits trailing spaces on blank lines
        // ("\n \n" instead of "\n\n"), which used to defeat both paragraph
        // detection and page-number stripping.
        assert_eq!(
            normalize_ocr("Ende. \n \n7 \n \nAnfang."),
            "Ende.\n\nAnfang."
        );
    }

    #[test]
    fn strips_a_trailing_page_number_with_no_blank_line_after_it() {
        // Regression: the old anchored regex required "\n\n" on both sides,
        // so a footer page number at the very end of the document survived.
        assert_eq!(
            normalize_ocr("Der Antrag wurde abgelehnt.\n\n12"),
            "Der Antrag wurde abgelehnt."
        );
    }

    #[test]
    fn strips_a_leading_page_number_with_no_blank_line_before_it() {
        // Regression: same defect, mirrored at the start of the document.
        assert_eq!(
            normalize_ocr("7\n\nDer Antrag wurde abgelehnt."),
            "Der Antrag wurde abgelehnt."
        );
    }
}
