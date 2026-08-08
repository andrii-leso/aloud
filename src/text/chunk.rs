/// Abbreviations that end in a period without ending a sentence.
const ABBREVIATIONS: &[&str] = &[
    "Dr.", "Mr.", "Mrs.", "Ms.", "Prof.", "St.", "Nr.", "z.B.", "u.a.", "ca.",
    "bzw.", "ggf.", "inkl.", "etc.", "e.g.", "i.e.", "vs.", "Abs.", "Art.",
];

fn ends_with_abbreviation(s: &str) -> bool {
    let trimmed = s.trim_end();
    ABBREVIATIONS.iter().any(|a| trimmed.ends_with(a))
}

/// True when the period at `idx` sits between two digits (a decimal).
fn is_decimal_point(chars: &[char], idx: usize) -> bool {
    idx > 0
        && idx + 1 < chars.len()
        && chars[idx - 1].is_ascii_digit()
        && chars[idx + 1].is_ascii_digit()
}

/// Splits into sentences for streaming synthesis. Paragraph breaks are
/// always boundaries; abbreviations and decimals are not.
pub fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split("\n\n") {
        let chars: Vec<char> = para.chars().collect();
        let mut start = 0usize;
        for i in 0..chars.len() {
            let c = chars[i];
            if c != '.' && c != '!' && c != '?' {
                continue;
            }
            if c == '.' && is_decimal_point(&chars, i) {
                continue;
            }
            let candidate: String = chars[start..=i].iter().collect();
            if c == '.' && ends_with_abbreviation(&candidate) {
                continue;
            }
            // Only break when followed by whitespace or end of paragraph.
            let followed_by_space = chars.get(i + 1).map_or(true, |n| n.is_whitespace());
            if !followed_by_space {
                continue;
            }
            let piece = candidate.trim();
            if !piece.is_empty() {
                out.push(piece.to_string());
            }
            start = i + 1;
        }
        if start < chars.len() {
            let tail: String = chars[start..].iter().collect();
            let tail = tail.trim();
            if !tail.is_empty() {
                out.push(tail.to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::split_sentences;

    #[test]
    fn splits_on_sentence_terminators() {
        assert_eq!(
            split_sentences("First one. Second one! Third one?"),
            vec!["First one.", "Second one!", "Third one?"]
        );
    }

    #[test]
    fn does_not_split_on_common_abbreviations() {
        assert_eq!(
            split_sentences("Send it to Dr. Meyer by Friday. Then wait."),
            vec!["Send it to Dr. Meyer by Friday.", "Then wait."]
        );
    }

    #[test]
    fn does_not_split_inside_decimals() {
        assert_eq!(
            split_sentences("The fee is 12.50 euros. Pay it."),
            vec!["The fee is 12.50 euros.", "Pay it."]
        );
    }

    #[test]
    fn treats_paragraph_breaks_as_boundaries() {
        assert_eq!(
            split_sentences("One paragraph\n\nAnother paragraph"),
            vec!["One paragraph", "Another paragraph"]
        );
    }

    #[test]
    fn returns_whole_text_when_it_has_no_terminator() {
        assert_eq!(split_sentences("no terminator here"), vec!["no terminator here"]);
    }

    #[test]
    fn empty_input_yields_no_chunks() {
        assert!(split_sentences("").is_empty());
        assert!(split_sentences("   \n  ").is_empty());
    }
}
