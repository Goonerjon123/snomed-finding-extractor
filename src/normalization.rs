use crate::model::SoapField;
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

/// Normalises raw SOAP text and then expands UK general-practice shorthand
/// ("c/o", "o/e", "h/o", "d&v", "sob", "fhx", ...) into the full clinical
/// phrases that terminology descriptions actually use. Expansion happens on
/// the text side only — terminology patterns are never expanded — and every
/// expanded byte keeps a map back to the original shorthand token, so match
/// spans still point at the text the clinician typed.
pub fn normalize_clinical_text(value: &str, field: SoapField) -> NormalizedText {
    expand_gp_shorthand(&normalize_with_map(value), field)
}

struct ShorthandRule {
    /// Token sequence as it appears in normalised text ("c/o" -> ["c", "o"]).
    source: &'static [&'static str],
    /// Replacement phrase in normalised form.
    replacement: &'static str,
    /// In the Objective field "FH" means fundal height or fetal heart, not
    /// family history, so ambiguous expansions are suppressed there.
    objective_safe: bool,
}

const SHORTHAND_RULES: &[ShorthandRule] = &[
    ShorthandRule {
        source: &["c", "o"],
        replacement: "complains of",
        objective_safe: true,
    },
    ShorthandRule {
        source: &["o", "e"],
        replacement: "on examination",
        objective_safe: true,
    },
    ShorthandRule {
        source: &["h", "o"],
        replacement: "history of",
        objective_safe: true,
    },
    ShorthandRule {
        source: &["d", "v"],
        replacement: "diarrhoea and vomiting",
        objective_safe: true,
    },
    ShorthandRule {
        source: &["d", "and", "v"],
        replacement: "diarrhoea and vomiting",
        objective_safe: true,
    },
    ShorthandRule {
        source: &["n", "v"],
        replacement: "nausea and vomiting",
        objective_safe: true,
    },
    ShorthandRule {
        source: &["n", "and", "v"],
        replacement: "nausea and vomiting",
        objective_safe: true,
    },
    ShorthandRule {
        source: &["soboe"],
        replacement: "shortness of breath on exertion",
        objective_safe: true,
    },
    ShorthandRule {
        source: &["sob"],
        replacement: "shortness of breath",
        objective_safe: true,
    },
    ShorthandRule {
        source: &["doe"],
        replacement: "dyspnoea on exertion",
        objective_safe: true,
    },
    ShorthandRule {
        source: &["fhx"],
        replacement: "family history",
        objective_safe: true,
    },
    ShorthandRule {
        source: &["fh"],
        replacement: "family history",
        objective_safe: false,
    },
    ShorthandRule {
        source: &["pmhx"],
        replacement: "past medical history",
        objective_safe: true,
    },
    ShorthandRule {
        source: &["pmh"],
        replacement: "past medical history",
        objective_safe: true,
    },
    ShorthandRule {
        source: &["hx"],
        replacement: "history",
        objective_safe: true,
    },
    ShorthandRule {
        source: &["nad"],
        replacement: "no abnormality detected",
        objective_safe: true,
    },
];

fn expand_gp_shorthand(normalized: &NormalizedText, field: SoapField) -> NormalizedText {
    let tokens = token_ranges(&normalized.text);
    let mut out = String::with_capacity(normalized.text.len());
    let mut starts = Vec::with_capacity(normalized.text.len());
    let mut ends = Vec::with_capacity(normalized.text.len());

    let mut index = 0;
    while index < tokens.len() {
        let rule = SHORTHAND_RULES
            .iter()
            .filter(|rule| rule.objective_safe || field != SoapField::Objective)
            .find(|rule| {
                rule.source.len() <= tokens.len() - index
                    && rule.source.iter().enumerate().all(|(offset, source)| {
                        let (start, end) = tokens[index + offset];
                        &normalized.text[start..end] == *source
                    })
            });

        match rule {
            Some(rule) => {
                let (first_start, _) = tokens[index];
                let (_, last_end) = tokens[index + rule.source.len() - 1];
                let original_start = normalized.byte_to_original_start[first_start];
                let original_end = normalized.byte_to_original_end[last_end - 1];
                push_separator(&mut out, &mut starts, &mut ends, original_start);
                for byte in rule.replacement.bytes() {
                    out.push(byte as char);
                    starts.push(original_start);
                    ends.push(original_end);
                }
                index += rule.source.len();
            }
            None => {
                let (start, end) = tokens[index];
                push_separator(
                    &mut out,
                    &mut starts,
                    &mut ends,
                    normalized.byte_to_original_start[start],
                );
                out.push_str(&normalized.text[start..end]);
                starts.extend_from_slice(&normalized.byte_to_original_start[start..end]);
                ends.extend_from_slice(&normalized.byte_to_original_end[start..end]);
                index += 1;
            }
        }
    }

    NormalizedText {
        text: out,
        byte_to_original_start: starts,
        byte_to_original_end: ends,
    }
}

fn push_separator(
    out: &mut String,
    starts: &mut Vec<usize>,
    ends: &mut Vec<usize>,
    original: usize,
) {
    if !out.is_empty() {
        out.push(' ');
        starts.push(original);
        ends.push(original);
    }
}

fn token_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut tokens = Vec::new();
    let mut start = None;
    for (idx, ch) in text.char_indices() {
        if ch == ' ' {
            if let Some(token_start) = start.take() {
                tokens.push((token_start, idx));
            }
        } else if start.is_none() {
            start = Some(idx);
        }
    }
    if let Some(token_start) = start {
        tokens.push((token_start, text.len()));
    }
    tokens
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

    #[test]
    fn expands_gp_shorthand_with_span_mapping() {
        let source = "c/o SOB at rest";
        let expanded = normalize_clinical_text(source, SoapField::History);

        assert_eq!(expanded.text, "complains of shortness of breath at rest");

        let start = expanded.text.find("shortness of breath").unwrap();
        let end = start + "shortness of breath".len();
        let (original_start, original_end) = expanded.original_range(start, end).unwrap();
        assert_eq!(&source[original_start..original_end], "SOB");
    }

    #[test]
    fn expands_two_token_shorthand_from_punctuated_forms() {
        assert_eq!(
            normalize_clinical_text("h/o D&V last week", SoapField::History).text,
            "history of diarrhoea and vomiting last week"
        );
    }

    #[test]
    fn expands_spelled_out_gp_shorthand_coordination() {
        assert_eq!(
            normalize_clinical_text("D and V overnight; N and V settled", SoapField::History).text,
            "diarrhoea and vomiting overnight nausea and vomiting settled"
        );
    }

    #[test]
    fn does_not_expand_fh_in_the_objective_field() {
        assert_eq!(
            normalize_clinical_text("FH 32cm", SoapField::Objective).text,
            "fh 32cm"
        );
        assert_eq!(
            normalize_clinical_text("FH: nil of note", SoapField::History).text,
            "family history nil of note"
        );
    }

    #[test]
    fn leaves_single_letters_alone_when_not_shorthand() {
        // "hep C or B" must not become "complains of"-style expansions.
        assert_eq!(
            normalize_clinical_text("hep C or B positive", SoapField::History).text,
            "hep c or b positive"
        );
    }
}
