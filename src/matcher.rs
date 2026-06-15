use crate::error::{ExtractorError, Result};
use crate::model::{MeasuredValue, SoapField};
use crate::normalization::{
    is_normalized_word_boundary, normalize_clinical_text, normalize_term, NormalizedText,
};
use crate::terminology::{is_blocked_common_term, TerminologyArtefact};
use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// A terminology term that was excluded from matching because it mapped to
/// more than one concept without a unique exact preferred term. Surfaced as a
/// build-time audit so terminology curators can see what the safety guard
/// silently removed, instead of discovering gaps only at runtime.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DroppedTerm {
    pub term: String,
    pub concept_id: String,
    pub preferred_term: String,
    pub source: String,
    pub competing_concept_count: usize,
}

#[derive(Debug, Clone)]
pub struct RawMatch {
    pub concept_id: String,
    pub preferred_term: String,
    pub field: SoapField,
    pub span_start: usize,
    pub span_end: usize,
    pub matched_text: String,
    pub normalized_match: String,
    pub pattern_source: String,
    pub value: Option<MeasuredValue>,
}

#[derive(Debug, Clone)]
struct PatternMeta {
    concept_id: String,
    preferred_term: String,
    pattern: String,
    source: String,
    requires_numeric_value: bool,
}

#[derive(Debug, Clone)]
struct PatternCandidate {
    concept_id: String,
    preferred_term: String,
    pattern: String,
    source: String,
    requires_numeric_value: bool,
}

#[derive(Debug, Clone)]
struct FlexiblePatternMeta {
    concept_id: String,
    preferred_term: String,
    pattern: String,
    source: String,
    tokens: Vec<String>,
    kind: FlexiblePatternKind,
}

#[derive(Debug, Clone, Copy)]
enum FlexiblePatternKind {
    BodySite,
    CoordinatedSharedHead,
}

#[derive(Debug, Clone)]
struct NormalizedToken<'a> {
    text: &'a str,
    start: usize,
    end: usize,
    original_start: usize,
    original_end: usize,
}

#[derive(Debug, Clone)]
pub struct TerminologyMatcher {
    automaton: AhoCorasick,
    patterns: Vec<PatternMeta>,
    flexible_patterns: Vec<FlexiblePatternMeta>,
    flexible_by_first_token: HashMap<String, Vec<usize>>,
    dropped_ambiguous: Vec<DroppedTerm>,
}

impl TerminologyMatcher {
    pub fn new(artefact: &TerminologyArtefact) -> Result<Self> {
        artefact.validate_runtime_terms()?;

        let mut pattern_strings = Vec::new();
        let mut patterns = Vec::new();
        let mut candidates = Vec::new();
        let mut concepts_by_term: HashMap<String, HashSet<String>> = HashMap::new();
        let mut numeric_concepts_by_term: HashMap<String, HashSet<String>> = HashMap::new();
        let mut exact_preferred_concepts_by_term: HashMap<String, HashSet<String>> = HashMap::new();
        let mut seen = HashSet::new();

        for concept in artefact.concepts.iter().filter(|concept| concept.active) {
            let normalized_preferred = normalize_term(&concept.preferred_term);
            for variant in concept.runtime_variants() {
                let normalized = normalize_term(&variant.text);
                if normalized.is_empty()
                    || is_blocked_common_term(
                        &normalized,
                        variant.allow_ambiguous || variant.requires_numeric_value,
                    )
                {
                    continue;
                }

                concepts_by_term
                    .entry(normalized.clone())
                    .or_default()
                    .insert(concept.concept_id.clone());
                if normalized == normalized_preferred {
                    exact_preferred_concepts_by_term
                        .entry(normalized.clone())
                        .or_default()
                        .insert(concept.concept_id.clone());
                }
                if variant.requires_numeric_value {
                    numeric_concepts_by_term
                        .entry(normalized.clone())
                        .or_default()
                        .insert(concept.concept_id.clone());
                }
                candidates.push(PatternCandidate {
                    concept_id: concept.concept_id.clone(),
                    preferred_term: concept.preferred_term.clone(),
                    pattern: normalized,
                    source: variant.source,
                    requires_numeric_value: variant.requires_numeric_value,
                });
            }
        }

        let mut flexible_patterns = Vec::new();
        let mut flexible_by_first_token: HashMap<String, Vec<usize>> = HashMap::new();
        let mut dropped_ambiguous = Vec::new();
        let mut dropped_seen = HashSet::new();
        for candidate in candidates {
            let competing = if candidate.requires_numeric_value {
                &numeric_concepts_by_term
            } else {
                &concepts_by_term
            }
            .get(&candidate.pattern)
            .map(|concepts| concepts.len())
            .unwrap_or(0);
            let is_ambiguous = competing > 1;
            let has_unique_exact_preferred = exact_preferred_concepts_by_term
                .get(&candidate.pattern)
                .map(|concepts| concepts.len() == 1 && concepts.contains(&candidate.concept_id))
                .unwrap_or(false);
            if is_ambiguous && !has_unique_exact_preferred {
                if dropped_seen.insert((candidate.concept_id.clone(), candidate.pattern.clone())) {
                    dropped_ambiguous.push(DroppedTerm {
                        term: candidate.pattern.clone(),
                        concept_id: candidate.concept_id.clone(),
                        preferred_term: candidate.preferred_term.clone(),
                        source: candidate.source.clone(),
                        competing_concept_count: competing,
                    });
                }
                continue;
            }

            let key = (candidate.concept_id.as_str(), candidate.pattern.as_str());
            if !seen.insert((key.0.to_string(), key.1.to_string())) {
                continue;
            }

            pattern_strings.push(candidate.pattern.clone());
            patterns.push(PatternMeta {
                concept_id: candidate.concept_id.clone(),
                preferred_term: candidate.preferred_term.clone(),
                pattern: candidate.pattern.clone(),
                source: candidate.source.clone(),
                requires_numeric_value: candidate.requires_numeric_value,
            });

            if !candidate.requires_numeric_value {
                if let Some(tokens) = flexible_body_site_pattern_tokens(&candidate.pattern) {
                    push_flexible_pattern(
                        &mut flexible_patterns,
                        &mut flexible_by_first_token,
                        &candidate,
                        tokens,
                        "flexible-body-site",
                        FlexiblePatternKind::BodySite,
                    );
                }
                if let Some(tokens) = coordinated_shared_head_pattern_tokens(&candidate.pattern) {
                    push_flexible_pattern(
                        &mut flexible_patterns,
                        &mut flexible_by_first_token,
                        &candidate,
                        tokens,
                        "coordinated-shared-head",
                        FlexiblePatternKind::CoordinatedSharedHead,
                    );
                }
            }
        }

        if patterns.is_empty() {
            return Err(ExtractorError::EmptyTerminology);
        }

        let automaton = AhoCorasickBuilder::new()
            .match_kind(MatchKind::LeftmostLongest)
            .build(pattern_strings)
            .map_err(|err| ExtractorError::Matcher(err.to_string()))?;

        dropped_ambiguous.sort_by(|a, b| {
            b.competing_concept_count
                .cmp(&a.competing_concept_count)
                .then_with(|| a.term.cmp(&b.term))
        });

        Ok(Self {
            automaton,
            patterns,
            flexible_patterns,
            flexible_by_first_token,
            dropped_ambiguous,
        })
    }

    /// Terms excluded from matching by the ambiguity guard, worst-first.
    pub fn dropped_ambiguous(&self) -> &[DroppedTerm] {
        &self.dropped_ambiguous
    }

    /// Finds terminology matches in one SOAP field. When `capture_values` is
    /// set (the observables extraction), each match also captures the numeric
    /// value and unit that follow it, so the EPR can fill an openEHR quantity
    /// without re-parsing the note.
    pub fn find_in_field(
        &self,
        field: SoapField,
        text: &str,
        capture_values: bool,
    ) -> Vec<RawMatch> {
        let normalized = normalize_clinical_text(text, field);
        let tokens = normalized_tokens(&normalized);
        let mut seen = HashSet::new();
        let mut matches = Vec::new();

        for found in self.automaton.find_iter(&normalized.text) {
            if !is_normalized_word_boundary(&normalized.text, found.start(), found.end()) {
                continue;
            }

            let Some((span_start, span_end)) =
                normalized.original_range(found.start(), found.end())
            else {
                continue;
            };

            let meta = &self.patterns[found.pattern().as_usize()];
            // Short numeric labels (T, P, ...) only match when a value follows;
            // observable matches additionally capture the value for the EPR.
            let value = if capture_values || meta.requires_numeric_value {
                capture_value_after(&normalized, text, &tokens, found.end())
            } else {
                None
            };
            if meta.requires_numeric_value && value.is_none() {
                continue;
            }
            if !acronym_match_casing_is_safe(&meta.source, &text[span_start..span_end]) {
                continue;
            }
            let key = (meta.concept_id.as_str(), span_start, span_end);
            if !seen.insert((key.0.to_string(), key.1, key.2)) {
                continue;
            }

            matches.push(RawMatch {
                concept_id: meta.concept_id.clone(),
                preferred_term: meta.preferred_term.clone(),
                field,
                span_start,
                span_end,
                matched_text: text[span_start..span_end].to_string(),
                normalized_match: meta.pattern.clone(),
                pattern_source: meta.source.clone(),
                value,
            });
        }

        for (token_index, token) in tokens.iter().enumerate() {
            for key in [token.text.to_string(), singular_token(token.text)] {
                let Some(pattern_indices) = self.flexible_by_first_token.get(&key) else {
                    continue;
                };

                for pattern_index in pattern_indices {
                    let meta = &self.flexible_patterns[*pattern_index];
                    let matched_range = match meta.kind {
                        FlexiblePatternKind::BodySite => {
                            find_flexible_body_site_match(&tokens, token_index, &meta.tokens)
                        }
                        FlexiblePatternKind::CoordinatedSharedHead => {
                            find_coordinated_shared_head_match(
                                text,
                                &tokens,
                                token_index,
                                &meta.tokens,
                            )
                        }
                    };
                    let Some((start, end)) = matched_range else {
                        continue;
                    };
                    let Some((span_start, span_end)) = normalized.original_range(start, end) else {
                        continue;
                    };
                    if !acronym_match_casing_is_safe(&meta.source, &text[span_start..span_end]) {
                        continue;
                    }

                    let key = (meta.concept_id.as_str(), span_start, span_end);
                    if !seen.insert((key.0.to_string(), key.1, key.2)) {
                        continue;
                    }

                    matches.push(RawMatch {
                        concept_id: meta.concept_id.clone(),
                        preferred_term: meta.preferred_term.clone(),
                        field,
                        span_start,
                        span_end,
                        matched_text: text[span_start..span_end].to_string(),
                        normalized_match: meta.pattern.clone(),
                        pattern_source: meta.source.clone(),
                        value: None,
                    });
                }
            }
        }

        matches
    }
}

fn acronym_match_casing_is_safe(source: &str, matched_text: &str) -> bool {
    if !source.contains("description-acronym") {
        return true;
    }

    let letters = matched_text
        .chars()
        .filter(|ch| ch.is_ascii_alphabetic())
        .collect::<Vec<_>>();
    if letters.is_empty() {
        return false;
    }
    if matched_text.chars().any(|ch| ch.is_ascii_digit()) {
        return true;
    }

    letters.len() >= 2 && letters.iter().all(|ch| ch.is_ascii_uppercase())
}

/// Filler words tolerated between a numeric label and its value, so
/// "BP today 128/82" and "BP of 128/82" capture 128/82 just like "BP 128/82".
const VALUE_FILLER: &[&str] = &[
    "today",
    "of",
    "is",
    "was",
    "now",
    "at",
    "currently",
    "measured",
    "recorded",
    "reading",
    "the",
    "this",
    "morning",
    "around",
    "approximately",
    "approx",
];

/// Unit tokens accepted immediately after a captured value. Restrictive on
/// purpose: anything not here (e.g. "on" in "98 on air") yields no unit rather
/// than a wrong one.
const KNOWN_UNITS: &[&str] = &[
    "%",
    "mmhg",
    "kg",
    "g",
    "lb",
    "lbs",
    "st",
    "cm",
    "mm",
    "m",
    "bpm",
    "c",
    "f",
    "celsius",
    "fahrenheit",
    "kpa",
    "mg",
    "mcg",
    "ml",
    "l",
    "mmol",
    "min",
    "kg/m2",
];

/// Captures the value (and unit, if any) following a numeric label, tolerating
/// a bounded run of filler words. Returns spans in the original text.
fn capture_value_after(
    normalized: &NormalizedText,
    text: &str,
    tokens: &[NormalizedToken<'_>],
    pattern_end: usize,
) -> Option<MeasuredValue> {
    let start_index = tokens.iter().position(|token| token.start >= pattern_end)?;

    for (offset, token) in tokens[start_index..].iter().enumerate() {
        if is_value_token(token.text) {
            let (value_start, value_end) = normalized.original_range(token.start, token.end)?;
            let (unit, span_end) = capture_unit_after(text, value_end);
            return Some(MeasuredValue {
                text: text[value_start..value_end].to_string(),
                unit,
                span_start: value_start,
                span_end,
            });
        }

        // Tolerate at most three filler words ("today", "of", ...) before the
        // value; anything else means no value belongs to this label.
        if offset >= 3 || !VALUE_FILLER.contains(&token.text) {
            return None;
        }
    }

    None
}

fn is_value_token(token: &str) -> bool {
    matches!(token.chars().next(), Some(ch) if ch.is_ascii_digit() || ch == '-' || ch == '+')
}

/// Reads a unit from the original text directly after a captured value,
/// allowing a single optional space and an optional degree sign before a
/// known unit token. Returns the unit (as typed) and the end of the value+unit
/// span; when no known unit follows, the span ends at the value.
fn capture_unit_after(text: &str, value_end: usize) -> (Option<String>, usize) {
    let rest = &text[value_end..];
    let space = rest.len() - rest.trim_start_matches(' ').len();
    // Only a single separating space is allowed before a unit.
    if space > 1 {
        return (None, value_end);
    }
    let after_space = &rest[space..];

    if let Some(stripped) = after_space.strip_prefix('%') {
        let _ = stripped;
        return (Some("%".to_string()), value_end + space + 1);
    }

    let degree = if after_space.starts_with('\u{b0}') {
        '\u{b0}'.len_utf8()
    } else {
        0
    };
    let unit_region = &after_space[degree..];
    let unit_len = unit_region
        .char_indices()
        .take_while(|(_, ch)| ch.is_ascii_alphabetic() || *ch == '/' || ch.is_ascii_digit())
        .map(|(idx, ch)| idx + ch.len_utf8())
        .last()
        .unwrap_or(0);
    if unit_len == 0 {
        return (None, value_end);
    }

    let raw_unit = &unit_region[..unit_len];
    if KNOWN_UNITS.contains(&raw_unit.to_ascii_lowercase().as_str()) {
        (
            Some(raw_unit.to_string()),
            value_end + space + degree + unit_len,
        )
    } else {
        (None, value_end)
    }
}

fn push_flexible_pattern(
    flexible_patterns: &mut Vec<FlexiblePatternMeta>,
    flexible_by_first_token: &mut HashMap<String, Vec<usize>>,
    candidate: &PatternCandidate,
    tokens: Vec<String>,
    source_suffix: &'static str,
    kind: FlexiblePatternKind,
) {
    let first_token = tokens[0].clone();
    let flexible_index = flexible_patterns.len();
    flexible_patterns.push(FlexiblePatternMeta {
        concept_id: candidate.concept_id.clone(),
        preferred_term: candidate.preferred_term.clone(),
        pattern: candidate.pattern.clone(),
        source: format!("{}:{source_suffix}", candidate.source),
        tokens,
        kind,
    });
    flexible_by_first_token
        .entry(first_token.clone())
        .or_default()
        .push(flexible_index);
    let singular_first = singular_token(&first_token);
    if singular_first != first_token {
        flexible_by_first_token
            .entry(singular_first)
            .or_default()
            .push(flexible_index);
    }
}

fn flexible_body_site_pattern_tokens(pattern: &str) -> Option<Vec<String>> {
    let tokens = pattern
        .split(' ')
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if tokens.len() < 3 || tokens.len() > 7 {
        return None;
    }
    if weak_flexible_start(tokens.first()?.as_str()) {
        return None;
    }
    let preposition_index = tokens
        .iter()
        .position(|token| flexible_body_site_preposition(token))?;
    if preposition_index == 0 || preposition_index + 1 >= tokens.len() {
        return None;
    }

    Some(tokens)
}

fn coordinated_shared_head_pattern_tokens(pattern: &str) -> Option<Vec<String>> {
    let tokens = pattern
        .split(' ')
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if tokens.len() != 2 {
        return None;
    }
    if weak_flexible_start(tokens[0].as_str()) || tokens[0].chars().count() < 4 {
        return None;
    }
    if tokens[1].chars().count() < 4 {
        return None;
    }

    Some(tokens)
}

fn weak_flexible_start(token: &str) -> bool {
    matches!(
        token,
        "ability"
            | "appearance"
            | "character"
            | "characteristic"
            | "feature"
            | "finding"
            | "function"
            | "measurement"
            | "observation"
            | "status"
    )
}

fn flexible_body_site_preposition(token: &str) -> bool {
    matches!(token, "of" | "on" | "from" | "over" | "in")
}

fn normalized_tokens(normalized: &NormalizedText) -> Vec<NormalizedToken<'_>> {
    let text = normalized.text.as_str();
    let mut tokens = Vec::new();
    let mut token_start = None;
    for (idx, ch) in text.char_indices() {
        if ch == ' ' {
            if let Some(start) = token_start.take() {
                let (original_start, original_end) = normalized
                    .original_range(start, idx)
                    .unwrap_or((start, idx));
                tokens.push(NormalizedToken {
                    text: &text[start..idx],
                    start,
                    end: idx,
                    original_start,
                    original_end,
                });
            }
        } else if token_start.is_none() {
            token_start = Some(idx);
        }
    }
    if let Some(start) = token_start {
        let (original_start, original_end) = normalized
            .original_range(start, text.len())
            .unwrap_or((start, text.len()));
        tokens.push(NormalizedToken {
            text: &text[start..],
            start,
            end: text.len(),
            original_start,
            original_end,
        });
    }

    tokens
}

fn find_flexible_body_site_match(
    tokens: &[NormalizedToken<'_>],
    start_index: usize,
    pattern_tokens: &[String],
) -> Option<(usize, usize)> {
    const MAX_EXTRA_TOKENS: usize = 4;

    if !token_matches(&pattern_tokens[0], tokens.get(start_index)?.text) {
        return None;
    }

    let mut search_from = start_index + 1;
    let mut extra_tokens = 0_usize;
    let mut end_index = start_index;
    for pattern_token in pattern_tokens.iter().skip(1) {
        let mut found_index = None;
        let search_limit = (search_from + MAX_EXTRA_TOKENS + 1).min(tokens.len());
        for (candidate_index, candidate_token) in tokens
            .iter()
            .enumerate()
            .take(search_limit)
            .skip(search_from)
        {
            if token_matches(pattern_token, candidate_token.text) {
                found_index = Some(candidate_index);
                break;
            }
        }

        let found_index = found_index?;
        extra_tokens += found_index.saturating_sub(search_from);
        if extra_tokens > MAX_EXTRA_TOKENS {
            return None;
        }

        search_from = found_index + 1;
        end_index = found_index;
    }

    Some((tokens[start_index].start, tokens[end_index].end))
}

fn find_coordinated_shared_head_match(
    original_text: &str,
    tokens: &[NormalizedToken<'_>],
    start_index: usize,
    pattern_tokens: &[String],
) -> Option<(usize, usize)> {
    const MAX_EXTRA_TOKENS: usize = 2;

    if pattern_tokens.len() != 2
        || !token_matches(&pattern_tokens[0], tokens.get(start_index)?.text)
    {
        return None;
    }

    let search_from = start_index + 1;
    let search_limit = (search_from + MAX_EXTRA_TOKENS + 1).min(tokens.len());
    for (candidate_index, candidate_token) in tokens
        .iter()
        .enumerate()
        .take(search_limit)
        .skip(search_from)
    {
        if !token_matches(&pattern_tokens[1], candidate_token.text) {
            continue;
        }

        let skipped = candidate_index.saturating_sub(search_from);
        if skipped == 0 {
            continue;
        }
        if !has_original_coordinator_between(
            original_text,
            tokens[start_index].original_end,
            candidate_token.original_start,
        ) {
            continue;
        }

        return Some((tokens[start_index].start, candidate_token.end));
    }

    None
}

fn has_original_coordinator_between(original_text: &str, start: usize, end: usize) -> bool {
    if start >= end || end > original_text.len() {
        return false;
    }
    let gap = original_text[start..end].to_ascii_lowercase();
    gap.contains('/')
        || gap.contains(',')
        || gap
            .split(|ch: char| !ch.is_ascii_alphanumeric())
            .any(|word| matches!(word, "and" | "or"))
}

fn token_matches(pattern_token: &str, text_token: &str) -> bool {
    pattern_token == text_token || singular_token(pattern_token) == singular_token(text_token)
}

fn singular_token(token: &str) -> String {
    if token.len() <= 3 {
        return token.to_string();
    }
    if let Some(stem) = token.strip_suffix("ies") {
        return format!("{stem}y");
    }
    for suffix in ["ches", "shes", "xes", "ses"] {
        if let Some(stem) = token.strip_suffix(suffix) {
            return format!("{stem}{}", &suffix[..suffix.len() - 2]);
        }
    }
    if let Some(stem) = token.strip_suffix('s') {
        return stem.to_string();
    }

    token.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminology::{ConceptEntry, TermVariant, TerminologyArtefact};

    fn artefact() -> TerminologyArtefact {
        TerminologyArtefact {
            schema_version: 1,
            terminology_version: "test".to_string(),
            source_release: "test".to_string(),
            refset_id: "test-refset".to_string(),
            generated_at_utc: "test".to_string(),
            concepts: vec![ConceptEntry {
                concept_id: "1000000001".to_string(),
                active: true,
                preferred_term: "Chest pain".to_string(),
                descriptions: vec![],
                variants: vec![TermVariant {
                    text: "chest pain".to_string(),
                    source: "fixture".to_string(),
                    description_id: None,
                    allow_ambiguous: false,
                    requires_numeric_value: false,
                }],
            }],
            artefact_hash: String::new(),
        }
    }

    #[test]
    fn finds_longest_normalized_span() {
        let matcher = TerminologyMatcher::new(&artefact()).unwrap();
        let matches = matcher.find_in_field(SoapField::Assessment, "CHEST-pain present", false);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].matched_text, "CHEST-pain");
    }

    #[test]
    fn skips_terms_that_map_to_multiple_concepts() {
        let mut artefact = artefact();
        artefact.concepts.push(ConceptEntry {
            concept_id: "1000000002".to_string(),
            active: true,
            preferred_term: "Chest pain".to_string(),
            descriptions: vec![],
            variants: vec![TermVariant {
                text: "chest pain".to_string(),
                source: "fixture".to_string(),
                description_id: None,
                allow_ambiguous: false,
                requires_numeric_value: false,
            }],
        });
        artefact.concepts.push(ConceptEntry {
            concept_id: "1000000003".to_string(),
            active: true,
            preferred_term: "Unique symptom".to_string(),
            descriptions: vec![],
            variants: vec![TermVariant {
                text: "unique symptom".to_string(),
                source: "fixture".to_string(),
                description_id: None,
                allow_ambiguous: false,
                requires_numeric_value: false,
            }],
        });

        let matcher = TerminologyMatcher::new(&artefact).unwrap();

        assert!(matcher
            .find_in_field(SoapField::History, "chest pain", false)
            .is_empty());
        assert_eq!(
            matcher
                .find_in_field(SoapField::History, "unique symptom", false)
                .len(),
            1
        );
    }

    #[test]
    fn keeps_unique_exact_preferred_term_when_variant_is_ambiguous() {
        let artefact = TerminologyArtefact {
            schema_version: 1,
            terminology_version: "test".to_string(),
            source_release: "test".to_string(),
            refset_id: "test-refset".to_string(),
            generated_at_utc: "test".to_string(),
            concepts: vec![
                ConceptEntry {
                    concept_id: "49727002".to_string(),
                    active: true,
                    preferred_term: "Cough".to_string(),
                    descriptions: vec![],
                    variants: vec![TermVariant {
                        text: "Cough".to_string(),
                        source: "fixture".to_string(),
                        description_id: None,
                        allow_ambiguous: false,
                        requires_numeric_value: false,
                    }],
                },
                ConceptEntry {
                    concept_id: "1000000004".to_string(),
                    active: true,
                    preferred_term: "Cough variant asthma".to_string(),
                    descriptions: vec![],
                    variants: vec![TermVariant {
                        text: "Cough".to_string(),
                        source: "fixture-derived".to_string(),
                        description_id: None,
                        allow_ambiguous: false,
                        requires_numeric_value: false,
                    }],
                },
            ],
            artefact_hash: String::new(),
        };

        let matcher = TerminologyMatcher::new(&artefact).unwrap();
        let matches = matcher.find_in_field(SoapField::History, "Cough for 3 months", false);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].concept_id, "49727002");
        assert_eq!(matches[0].preferred_term, "Cough");
    }

    #[test]
    fn derived_acronyms_require_uppercase_evidence_without_digits() {
        let artefact = TerminologyArtefact {
            schema_version: 1,
            terminology_version: "test".to_string(),
            source_release: "test".to_string(),
            refset_id: "test-refset".to_string(),
            generated_at_utc: "test".to_string(),
            concepts: vec![ConceptEntry {
                concept_id: "1000000099".to_string(),
                active: true,
                preferred_term: "Alpha beta condition".to_string(),
                descriptions: vec![],
                variants: vec![TermVariant {
                    text: "ABC".to_string(),
                    source: "openehr-description-acronym".to_string(),
                    description_id: None,
                    allow_ambiguous: true,
                    requires_numeric_value: false,
                }],
            }],
            artefact_hash: String::new(),
        };
        let matcher = TerminologyMatcher::new(&artefact).unwrap();

        assert!(matcher
            .find_in_field(SoapField::History, "abc noted", false)
            .is_empty());
        assert_eq!(
            matcher
                .find_in_field(SoapField::History, "ABC noted", false)
                .len(),
            1
        );
    }
}
