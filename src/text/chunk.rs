/// Abbreviations that end in a period without ending a sentence.
const ABBREVIATIONS: &[&str] = &[
    "Dr.", "Mr.", "Mrs.", "Ms.", "Prof.", "St.", "Nr.", "z.B.", "u.a.", "ca.",
    "bzw.", "ggf.", "inkl.", "etc.", "e.g.", "i.e.", "vs.", "Abs.", "Art.",
];

/// Maximum chunk length in characters. Measured on M1 Mac: a 95-character
/// sentence synthesizes in ~1.5s (warm engine), so 120 chars fits within
/// the 2.0s hard constraint for time-to-first-audio, even accounting for
/// cold starts and OS jitter.
const MAX_CHUNK_CHARS: usize = 120;

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

/// Splits a long chunk at a natural break point within the budget.
/// Searches the first MAX_CHUNK_CHARS chars for (in order):
/// 1. Last comma, semicolon, or colon (pause point)
/// 2. Last whitespace (word boundary)
/// 3. Hard cut at MAX_CHUNK_CHARS (single unbroken token)
///
/// Termination proof: Each invocation reduces the remaining length.
/// - Rules 1-2: We find a separator within the budget and split before it,
///   leaving a strictly shorter tail (bounded by MAX_CHUNK_CHARS at most).
/// - Rule 3: We cut exactly 120 chars, leaving a non-empty tail for the
///   next iteration (the original chunk must be > 120 to enter this function).
/// Thus the algorithm cannot hang; every iteration makes progress.
fn split_long_chunk(chunk: &str) -> Vec<String> {
    let chars: Vec<char> = chunk.chars().collect();
    let mut out = Vec::new();

    let mut pos = 0;
    while pos < chars.len() {
        let remaining = chars.len() - pos;
        if remaining <= MAX_CHUNK_CHARS {
            // The tail fits in the budget; emit as-is.
            let tail: String = chars[pos..].iter().collect();
            let trimmed = tail.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
            break;
        }

        // Too long. Search within the budget for a break point.
        let budget_end = (pos + MAX_CHUNK_CHARS).min(chars.len());
        let window = &chars[pos..budget_end];

        // Try to find the last comma, semicolon, or colon (rule 1).
        if let Some(sep_idx) = window
            .iter()
            .rposition(|&c| c == ',' || c == ';' || c == ':')
        {
            let cut = pos + sep_idx;
            let chunk_text: String = chars[pos..=cut].iter().collect();
            let trimmed = chunk_text.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
            pos = cut + 1;
            // Skip following whitespace to start the next chunk cleanly.
            while pos < chars.len() && chars[pos].is_whitespace() {
                pos += 1;
            }
            continue;
        }

        // No punctuation; try the last whitespace (rule 2).
        if let Some(ws_idx) = window.iter().rposition(|c| c.is_whitespace()) {
            let cut = pos + ws_idx;
            let chunk_text: String = chars[pos..cut].iter().collect();
            let trimmed = chunk_text.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
            pos = cut + 1;
            // Skip following whitespace to start the next chunk cleanly.
            while pos < chars.len() && chars[pos].is_whitespace() {
                pos += 1;
            }
            continue;
        }

        // Single unbroken token longer than budget; hard cut (rule 3).
        let cut = pos + MAX_CHUNK_CHARS;
        let chunk_text: String = chars[pos..cut].iter().collect();
        let trimmed = chunk_text.trim();
        if !trimmed.is_empty() {
            out.push(trimmed.to_string());
        }
        pos = cut;
    }

    out
}

/// Splits into sentences for streaming synthesis. Paragraph breaks are
/// always boundaries; abbreviations and decimals are not. Very long chunks
/// are further split at natural pause points to meet latency constraints.
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

    // Post-process: split any chunk longer than MAX_CHUNK_CHARS.
    let mut final_out = Vec::new();
    for chunk in out {
        if chunk.chars().count() <= MAX_CHUNK_CHARS {
            final_out.push(chunk);
        } else {
            let split_chunks = split_long_chunk(&chunk);
            final_out.extend(split_chunks);
        }
    }

    final_out
}

#[cfg(test)]
mod tests {
    use super::split_sentences;

    // Original six tests — must still pass unchanged.

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

    // New regression tests for the safety valve.

    #[test]
    fn handles_terminator_free_long_run() {
        let long_run = "The quick brown fox jumps over the lazy dog and then continues with a very long sentence that has absolutely no terminators or punctuation marks whatsoever so it should keep going and going and going until we get to five hundred characters or so which is definitely longer than our budget but we need to make sure it splits properly into reasonable chunks without any sentence endings at all just one giant run of words that never stop";
        let result = split_sentences(long_run);
        assert!(result.len() > 1, "Long terminator-free run should produce multiple chunks");
        for chunk in &result {
            assert!(
                chunk.chars().count() <= 120,
                "Every chunk must be ≤120 chars, but got: {}",
                chunk.chars().count()
            );
        }
    }

    #[test]
    fn handles_abbreviation_dense_text() {
        let abbr_dense = "Dr. St. Prof. Mr. Meyer und Dr. Schmidt und Prof. Weber und Mr. Brown besuchen St. Petersburg zusammen mit Dr. Fischer und zusätzlich noch Mr. Wilson.";
        let result = split_sentences(abbr_dense);
        assert!(result.len() > 1, "Abbreviation-dense text should produce multiple chunks");
        for chunk in &result {
            assert!(
                chunk.chars().count() <= 120,
                "Every chunk must be ≤120 chars, but got: {}",
                chunk.chars().count()
            );
        }
    }

    #[test]
    fn splits_comma_rich_sentence_at_comma() {
        let comma_rich = "The first part of the sentence, which is quite long and contains many words, but then continues with a comma, and then more text follows after the comma, making the whole thing very long indeed.";
        let result = split_sentences(comma_rich);
        for chunk in &result {
            assert!(
                chunk.chars().count() <= 120,
                "Every chunk must be ≤120 chars, but got: {}",
                chunk.chars().count()
            );
        }
        // At least one chunk should end with a comma (proving we split at punctuation).
        assert!(
            result.iter().any(|c| c.ends_with(',')),
            "Should split at comma, at least one chunk should end with comma"
        );
    }

    #[test]
    fn handles_non_ascii_ukrainian() {
        let ukrainian = "Заявники мають подати документи. Їхні справи буде закрито.";
        let result = split_sentences(ukrainian);
        assert_eq!(result.len(), 2, "Should produce exactly two chunks from two sentences");
        assert_eq!(result[0], "Заявники мають подати документи.");
        assert_eq!(result[1], "Їхні справи буде закрито.");
        // Verify no panic and correct boundaries.
        for chunk in &result {
            assert!(chunk.chars().count() > 0, "No empty chunks");
        }
    }

    #[test]
    fn handles_non_ascii_german() {
        let german = "Üben Sie regelmäßig mit Ärztinnen und Ärzte. Die Überprüfung erfolgt später.";
        let result = split_sentences(german);
        assert_eq!(result.len(), 2, "Should produce exactly two chunks from two sentences");
        // Verify no panic and correct boundaries.
        for chunk in &result {
            assert!(chunk.chars().count() > 0, "No empty chunks");
        }
    }

    #[test]
    fn handles_non_ascii_long_no_terminator_ukrainian() {
        // 300+ char Ukrainian text with no sentence terminator. Forces split_long_chunk
        // to run with multi-byte Cyrillic throughout. Byte-indexing would corrupt or panic.
        let ukrainian = "Заявники, які мають подати документи, включаючи Петра Степаненко, Марію Коваленко, Ольгу Шевченко, Василя Грінченко, Софію Федоренко, Ярослава Бондаренко, Галину Сідоренко, Тетяну Литвиненко, та багато інших осіб, мають бути готові до перевірки всіх необхідних матеріалів";
        
        let result = split_sentences(ukrainian);
        
        // Must split into multiple chunks (no terminator forces split_long_chunk)
        assert!(
            result.len() > 1,
            "Long no-terminator Ukrainian should produce multiple chunks"
        );
        
        // Each chunk must be ≤120 chars
        for (i, chunk) in result.iter().enumerate() {
            let char_count = chunk.chars().count();
            assert!(
                char_count <= 120,
                "Chunk {} has {} chars (limit 120)",
                i,
                char_count
            );
        }
        
        // Round-trip: verify no characters are corrupted or lost.
        // Compare non-whitespace character sequences; split_long_chunk trims whitespace.
        let original_chars: Vec<char> = ukrainian
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let rejoined_chars: Vec<char> = result
            .iter()
            .flat_map(|s| s.chars())
            .filter(|c| !c.is_whitespace())
            .collect();
        assert_eq!(
            original_chars, rejoined_chars,
            "Round-trip should preserve all characters (byte-indexing would corrupt)"
        );
    }
}
