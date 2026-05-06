use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedText {
    pub text: String,
    byte_to_original_start: Vec<usize>,
    byte_to_original_end: Vec<usize>,
}

impl NormalizedText {
    pub fn original_range(
        &self,
        normalized_start: usize,
        normalized_end: usize,
    ) -> Option<(usize, usize)> {
        if normalized_start >= normalized_end || normalized_end > self.byte_to_original_end.len() {
            return None;
        }

        Some((
            self.byte_to_original_start[normalized_start],
            self.byte_to_original_end[normalized_end - 1],
        ))
    }
}

pub fn normalize_term(value: &str) -> String {
    normalize_with_map(value).text
}

pub fn normalize_with_map(value: &str) -> NormalizedText {
    let chars: Vec<(usize, char)> = value.char_indices().collect();
    let mut out = String::with_capacity(value.len());
    let mut starts = Vec::with_capacity(value.len());
    let mut ends = Vec::with_capacity(value.len());
    let mut pending_separator: Option<(usize, usize)> = None;
    let mut emitted_token = false;

    for (idx, (original_start, ch)) in chars.iter().copied().enumerate() {
        let original_end = chars
            .get(idx + 1)
            .map(|(next_start, _)| *next_start)
            .unwrap_or(value.len());
        let previous_ch = idx
            .checked_sub(1)
            .and_then(|previous_idx| chars.get(previous_idx).map(|(_, ch)| *ch));
        let next_ch = chars.get(idx + 1).map(|(_, ch)| *ch);

        let mut emitted_this_char = false;
        if is_numeric_symbol(ch, previous_ch, next_ch) {
            if let Some((sep_start, sep_end)) = pending_separator.take() {
                push_mapped_char(&mut out, &mut starts, &mut ends, ' ', sep_start, sep_end);
            }
            push_mapped_char(
                &mut out,
                &mut starts,
                &mut ends,
                ch,
                original_start,
                original_end,
            );
            emitted_token = true;
            continue;
        }

        for decomposed in ch.nfkd() {
            if is_combining_mark(decomposed) {
                continue;
            }

            for lowered in decomposed.to_lowercase() {
                if lowered.is_alphanumeric() {
                    if let Some((sep_start, sep_end)) = pending_separator.take() {
                        push_mapped_char(&mut out, &mut starts, &mut ends, ' ', sep_start, sep_end);
                    }
                    push_mapped_char(
                        &mut out,
                        &mut starts,
                        &mut ends,
                        lowered,
                        original_start,
                        original_end,
                    );
                    emitted_token = true;
                    emitted_this_char = true;
                }
            }
        }

        if !emitted_this_char && emitted_token {
            pending_separator = Some((original_start, original_end));
        }
    }

    NormalizedText {
        text: out,
        byte_to_original_start: starts,
        byte_to_original_end: ends,
    }
}

pub fn is_normalized_word_boundary(text: &str, start: usize, end: usize) -> bool {
    let before_ok = start == 0 || text[..start].ends_with(' ');
    let after_ok = end == text.len() || text[end..].starts_with(' ');
    before_ok && after_ok
}

fn push_mapped_char(
    out: &mut String,
    starts: &mut Vec<usize>,
    ends: &mut Vec<usize>,
    ch: char,
    original_start: usize,
    original_end: usize,
) {
    let mut encoded = [0_u8; 4];
    let width = ch.encode_utf8(&mut encoded).len();
    out.push(ch);
    for _ in 0..width {
        starts.push(original_start);
        ends.push(original_end);
    }
}

fn is_combining_mark(ch: char) -> bool {
    ('\u{0300}'..='\u{036f}').contains(&ch)
        || ('\u{1ab0}'..='\u{1aff}').contains(&ch)
        || ('\u{1dc0}'..='\u{1dff}').contains(&ch)
        || ('\u{20d0}'..='\u{20ff}').contains(&ch)
        || ('\u{fe20}'..='\u{fe2f}').contains(&ch)
}

fn is_numeric_symbol(ch: char, previous: Option<char>, next: Option<char>) -> bool {
    match ch {
        '-' | '+' => {
            next.map(|next| next.is_ascii_digit()).unwrap_or(false)
                || previous
                    .map(|previous| previous.is_ascii_digit())
                    .unwrap_or(false)
                    && next.map(|next| next.is_ascii_digit()).unwrap_or(false)
        }
        '.' | '/' => {
            previous
                .map(|previous| previous.is_ascii_digit())
                .unwrap_or(false)
                && next.map(|next| next.is_ascii_digit()).unwrap_or(false)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_case_punctuation_and_spacing() {
        assert_eq!(normalize_term("  Chest-pain!! "), "chest pain");
    }

    #[test]
    fn preserves_original_spans_after_normalization() {
        let source = "No CHEST-pain today";
        let normalized = normalize_with_map(source);
        let start = normalized.text.find("chest pain").unwrap();
        let end = start + "chest pain".len();
        let (original_start, original_end) = normalized.original_range(start, end).unwrap();

        assert_eq!(&source[original_start..original_end], "CHEST-pain");
    }

    #[test]
    fn preserves_numeric_signs_and_ranges() {
        assert_eq!(
            normalize_term("-1 level and 0-5 mitoses"),
            "-1 level and 0-5 mitoses"
        );
    }
}
