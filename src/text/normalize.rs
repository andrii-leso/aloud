use regex::Regex;
use std::sync::OnceLock;

fn re_hyphen_break() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // word- \n word  ->  wordword   (single newline only)
    R.get_or_init(|| Regex::new(r"(\w)-\n(?:[ \t]*)(\w)").unwrap())
}

fn re_page_number() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // A line containing only digits, surrounded by blank lines.
    R.get_or_init(|| Regex::new(r"\n\n[ \t]*\d{1,4}[ \t]*\n\n").unwrap())
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

/// Cleans OCR layout artifacts so the text reads as prose.
/// Order matters: hyphens before soft breaks, page numbers before either
/// would destroy the blank lines that identify them.
pub fn normalize_ocr(input: &str) -> String {
    let mut s = input.replace("\r\n", "\n");

    // Strip page numbers. Run repeatedly to handle consecutive page markers
    // (overlapping matches that a single replace_all would miss).
    while re_page_number().is_match(&s) {
        s = re_page_number().replace_all(&s, "\n\n").into_owned();
    }

    s = re_hyphen_break().replace_all(&s, "$1$2").into_owned();

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
        assert_eq!(normalize_ocr(input), "a well-known case-\n\nNext paragraph.");
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
        assert_eq!(
            normalize_ocr("End.\n\n7\n\n8\n\nStart."),
            "End.\n\nStart."
        );
    }
}
