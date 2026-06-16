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

#[derive(Debug, Clone)]
struct MorphPatternMeta {
    concept_id: String,
    preferred_term: String,
    pattern: String,
    source: String,
    tokens: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
enum FlexiblePatternKind {
    BodySite,
    BodySiteThenHead,
    ClinicalDescriptorFinal,
    CoordinatedSharedHead,
    SiteHeadReordered,
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
    morph_patterns: Vec<MorphPatternMeta>,
    morph_by_first_key: HashMap<String, Vec<usize>>,
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
        let mut official_concepts_by_term: HashMap<String, HashSet<String>> = HashMap::new();
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
                if is_official_term_source(&variant.source) {
                    official_concepts_by_term
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
        let mut accepted_candidates = Vec::new();
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
            let has_unique_official_term = official_concepts_by_term
                .get(&candidate.pattern)
                .map(|concepts| concepts.len() == 1 && concepts.contains(&candidate.concept_id))
                .unwrap_or(false);
            if is_ambiguous && !has_unique_exact_preferred && !has_unique_official_term {
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

            accepted_candidates.push(candidate);
        }

        let mut morph_concepts_by_signature: HashMap<String, HashSet<String>> = HashMap::new();
        for candidate in accepted_candidates
            .iter()
            .filter(|candidate| !candidate.requires_numeric_value)
        {
            if let Some(signature) = morph_signature(&candidate.pattern) {
                morph_concepts_by_signature
                    .entry(signature)
                    .or_default()
                    .insert(candidate.concept_id.clone());
            }
        }

        let mut morph_patterns = Vec::new();
        let mut morph_by_first_key: HashMap<String, Vec<usize>> = HashMap::new();
        let mut morph_seen = HashSet::new();

        for candidate in accepted_candidates {
            pattern_strings.push(candidate.pattern.clone());
            patterns.push(PatternMeta {
                concept_id: candidate.concept_id.clone(),
                preferred_term: candidate.preferred_term.clone(),
                pattern: candidate.pattern.clone(),
                source: candidate.source.clone(),
                requires_numeric_value: candidate.requires_numeric_value,
            });

            if !candidate.requires_numeric_value {
                if let Some(tokens) = morph_pattern_tokens(&candidate.pattern) {
                    let signature = tokens
                        .iter()
                        .map(|token| morphology_signature_token(token))
                        .collect::<Vec<_>>()
                        .join(" ");
                    let is_morph_ambiguous = morph_concepts_by_signature
                        .get(&signature)
                        .map(|concepts| concepts.len() > 1)
                        .unwrap_or(false);
                    if !is_morph_ambiguous
                        && morph_seen.insert((candidate.concept_id.clone(), signature))
                    {
                        push_morph_pattern(
                            &mut morph_patterns,
                            &mut morph_by_first_key,
                            &candidate,
                            tokens,
                        );
                    }
                }
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
                if let Some(tokens) = body_site_then_head_pattern_tokens(&candidate.pattern) {
                    push_flexible_pattern(
                        &mut flexible_patterns,
                        &mut flexible_by_first_token,
                        &candidate,
                        tokens,
                        "body-site-then-head",
                        FlexiblePatternKind::BodySiteThenHead,
                    );
                }
                if let Some(tokens) = body_site_descriptor_pattern_tokens(&candidate.pattern) {
                    push_flexible_pattern(
                        &mut flexible_patterns,
                        &mut flexible_by_first_token,
                        &candidate,
                        tokens,
                        "body-site-descriptor",
                        FlexiblePatternKind::BodySiteThenHead,
                    );
                }
                if let Some(tokens) = clinical_descriptor_final_pattern_tokens(&candidate.pattern) {
                    push_flexible_pattern(
                        &mut flexible_patterns,
                        &mut flexible_by_first_token,
                        &candidate,
                        tokens,
                        "clinical-descriptor-final",
                        FlexiblePatternKind::ClinicalDescriptorFinal,
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
                if let Some(tokens) = site_head_reordered_pattern_tokens(&candidate.pattern) {
                    push_flexible_pattern(
                        &mut flexible_patterns,
                        &mut flexible_by_first_token,
                        &candidate,
                        tokens,
                        "reordered-site-head",
                        FlexiblePatternKind::SiteHeadReordered,
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
            morph_patterns,
            morph_by_first_key,
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
            if has_hard_boundary_between(text, span_start, span_end) {
                continue;
            }

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

        self.add_morphological_matches(&tokens, &normalized, text, field, &mut seen, &mut matches);

        for (token_index, token) in tokens.iter().enumerate() {
            for key in [token.text.to_string(), singular_token(token.text)] {
                let Some(pattern_indices) = self.flexible_by_first_token.get(&key) else {
                    continue;
                };

                for pattern_index in pattern_indices {
                    let meta = &self.flexible_patterns[*pattern_index];
                    let matched_range = match meta.kind {
                        FlexiblePatternKind::BodySite => {
                            find_flexible_body_site_match(text, &tokens, token_index, &meta.tokens)
                        }
                        FlexiblePatternKind::BodySiteThenHead => {
                            find_body_site_then_head_match(text, &tokens, token_index, &meta.tokens)
                        }
                        FlexiblePatternKind::ClinicalDescriptorFinal => {
                            find_clinical_descriptor_final_match(
                                text,
                                &tokens,
                                token_index,
                                &meta.tokens,
                            )
                        }
                        FlexiblePatternKind::CoordinatedSharedHead => {
                            find_coordinated_shared_head_match(
                                text,
                                &tokens,
                                token_index,
                                &meta.tokens,
                            )
                        }
                        FlexiblePatternKind::SiteHeadReordered => {
                            find_site_head_reordered_match(text, &tokens, token_index, &meta.tokens)
                        }
                    };
                    let Some((start, end)) = matched_range else {
                        continue;
                    };
                    let Some((span_start, span_end)) = normalized.original_range(start, end) else {
                        continue;
                    };
                    if has_hard_boundary_between(text, span_start, span_end) {
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
                        value: None,
                    });
                }
            }
        }

        remove_subsumed_overlapping_matches(&mut matches);

        matches
    }

    fn add_morphological_matches(
        &self,
        tokens: &[NormalizedToken<'_>],
        normalized: &NormalizedText,
        text: &str,
        field: SoapField,
        seen: &mut HashSet<(String, usize, usize)>,
        matches: &mut Vec<RawMatch>,
    ) {
        let mut candidates = Vec::new();
        let mut candidate_seen = HashSet::new();

        for (token_index, token) in tokens.iter().enumerate() {
            for key in morphology_lookup_keys(token.text) {
                let Some(pattern_indices) = self.morph_by_first_key.get(&key) else {
                    continue;
                };

                for pattern_index in pattern_indices {
                    if !candidate_seen.insert((*pattern_index, token_index)) {
                        continue;
                    }

                    let meta = &self.morph_patterns[*pattern_index];
                    if token_index + meta.tokens.len() > tokens.len() {
                        continue;
                    }

                    let text_tokens = &tokens[token_index..token_index + meta.tokens.len()];
                    let mut changed = false;
                    let matched = meta.tokens.iter().zip(text_tokens.iter()).all(
                        |(pattern_token, text_token)| {
                            let token_matched = token_matches(pattern_token, text_token.text);
                            changed |= token_matched && pattern_token != text_token.text;
                            token_matched
                        },
                    );
                    if !matched || !changed {
                        continue;
                    }

                    let start = text_tokens[0].start;
                    let end = text_tokens[text_tokens.len() - 1].end;
                    let Some((span_start, span_end)) = normalized.original_range(start, end) else {
                        continue;
                    };
                    if has_hard_boundary_between(text, span_start, span_end) {
                        continue;
                    }
                    if !acronym_match_casing_is_safe(&meta.source, &text[span_start..span_end]) {
                        continue;
                    }

                    candidates.push(RawMatch {
                        concept_id: meta.concept_id.clone(),
                        preferred_term: meta.preferred_term.clone(),
                        field,
                        span_start,
                        span_end,
                        matched_text: text[span_start..span_end].to_string(),
                        normalized_match: meta.pattern.clone(),
                        pattern_source: format!("{}:morphology", meta.source),
                        value: None,
                    });
                }
            }
        }

        candidates.sort_by(|a, b| {
            a.span_start
                .cmp(&b.span_start)
                .then_with(|| (b.span_end - b.span_start).cmp(&(a.span_end - a.span_start)))
                .then_with(|| a.concept_id.cmp(&b.concept_id))
        });

        let mut occupied = Vec::<(usize, usize)>::new();
        for candidate in candidates {
            let candidate_len = candidate.span_end - candidate.span_start;
            if occupied
                .iter()
                .any(|span| spans_overlap(*span, (candidate.span_start, candidate.span_end)))
            {
                continue;
            }
            if matches.iter().any(|existing| {
                spans_overlap(
                    (existing.span_start, existing.span_end),
                    (candidate.span_start, candidate.span_end),
                ) && existing.span_end - existing.span_start >= candidate_len
            }) {
                continue;
            }

            let key = (
                candidate.concept_id.as_str(),
                candidate.span_start,
                candidate.span_end,
            );
            if !seen.insert((key.0.to_string(), key.1, key.2)) {
                continue;
            }

            occupied.push((candidate.span_start, candidate.span_end));
            matches.push(candidate);
        }
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

fn is_official_term_source(source: &str) -> bool {
    matches!(
        source,
        "preferred_term"
            | "openehr-display"
            | "openehr-description-preferred"
            | "openehr-description-synonym"
            | "openehr-description-expansion"
    )
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

fn push_morph_pattern(
    morph_patterns: &mut Vec<MorphPatternMeta>,
    morph_by_first_key: &mut HashMap<String, Vec<usize>>,
    candidate: &PatternCandidate,
    tokens: Vec<String>,
) {
    let morph_index = morph_patterns.len();
    for key in morphology_lookup_keys(&tokens[0]) {
        morph_by_first_key.entry(key).or_default().push(morph_index);
    }
    morph_patterns.push(MorphPatternMeta {
        concept_id: candidate.concept_id.clone(),
        preferred_term: candidate.preferred_term.clone(),
        pattern: candidate.pattern.clone(),
        source: candidate.source.clone(),
        tokens,
    });
}

fn morph_pattern_tokens(pattern: &str) -> Option<Vec<String>> {
    let tokens = pattern
        .split(' ')
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if tokens.is_empty() || tokens.len() > 8 {
        return None;
    }
    if tokens
        .iter()
        .any(|token| token.chars().any(|ch| ch.is_ascii_digit()))
    {
        return None;
    }

    Some(tokens)
}

fn morph_signature(pattern: &str) -> Option<String> {
    Some(
        morph_pattern_tokens(pattern)?
            .iter()
            .map(|token| morphology_signature_token(token))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

fn morphology_signature_token(token: &str) -> String {
    inflection_root(token).unwrap_or_else(|| singular_token(token))
}

fn morphology_lookup_keys(token: &str) -> Vec<String> {
    let mut keys = Vec::new();
    push_unique_key(&mut keys, token.to_string());
    push_unique_key(&mut keys, singular_token(token));
    if let Some(root) = inflection_root(token) {
        push_unique_key(&mut keys, root);
    }
    keys
}

fn push_unique_key(keys: &mut Vec<String>, key: String) {
    if !key.is_empty() && !keys.iter().any(|existing| existing == &key) {
        keys.push(key);
    }
}

fn spans_overlap(left: (usize, usize), right: (usize, usize)) -> bool {
    left.0 < right.1 && right.0 < left.1
}

fn remove_subsumed_overlapping_matches(matches: &mut Vec<RawMatch>) {
    let remove = (0..matches.len())
        .filter(|&index| {
            matches.iter().enumerate().any(|(other_index, other)| {
                index != other_index
                    && other.span_end - other.span_start
                        >= matches[index].span_end - matches[index].span_start
                    && spans_overlap(
                        (matches[index].span_start, matches[index].span_end),
                        (other.span_start, other.span_end),
                    )
                    && qualified_term_subsumes(
                        &other.normalized_match,
                        &matches[index].normalized_match,
                    )
            })
        })
        .collect::<HashSet<_>>();

    if remove.is_empty() {
        return;
    }

    let mut index = 0_usize;
    matches.retain(|_| {
        let keep = !remove.contains(&index);
        index += 1;
        keep
    });
}

fn qualified_term_subsumes(longer: &str, shorter: &str) -> bool {
    if longer == shorter {
        return false;
    }
    let longer_tokens = longer.split(' ').collect::<Vec<_>>();
    let shorter_tokens = shorter.split(' ').collect::<Vec<_>>();
    if shorter_tokens.is_empty() || shorter_tokens.len() >= longer_tokens.len() {
        return false;
    }

    if longer_tokens.starts_with(&shorter_tokens) {
        let next = longer_tokens[shorter_tokens.len()];
        return !matches!(next, "and" | "or");
    }
    if longer_tokens.ends_with(&shorter_tokens) {
        let previous = longer_tokens[longer_tokens.len() - shorter_tokens.len() - 1];
        return !matches!(previous, "and" | "or");
    }

    false
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

fn body_site_then_head_pattern_tokens(pattern: &str) -> Option<Vec<String>> {
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

    let head = &tokens[..preposition_index];
    let site = &tokens[preposition_index + 1..];
    if head.len() > 3 || !likely_body_site_tokens(site) {
        return None;
    }

    let mut reordered = Vec::with_capacity(head.len() + site.len());
    reordered.extend(site.iter().cloned());
    reordered.extend(head.iter().cloned());
    Some(reordered)
}

fn body_site_descriptor_pattern_tokens(pattern: &str) -> Option<Vec<String>> {
    let tokens = pattern
        .split(' ')
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if tokens.len() < 2 || tokens.len() > 5 {
        return None;
    }
    let descriptor = tokens.last()?;
    if !reorderable_site_descriptor(descriptor)
        || !likely_body_site_tokens(&tokens[..tokens.len() - 1])
    {
        return None;
    }
    Some(tokens)
}

fn clinical_descriptor_final_pattern_tokens(pattern: &str) -> Option<Vec<String>> {
    let tokens = pattern
        .split(' ')
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if tokens.len() < 2 || tokens.len() > 4 {
        return None;
    }

    let descriptor = tokens.first()?;
    let noun_phrase = &tokens[1..];
    if !reorderable_clinical_descriptor(descriptor) || !likely_clinical_noun_phrase(noun_phrase) {
        return None;
    }

    let mut reordered = Vec::with_capacity(tokens.len());
    reordered.extend(noun_phrase.iter().cloned());
    reordered.push(descriptor.clone());
    Some(reordered)
}

fn reorderable_clinical_descriptor(token: &str) -> bool {
    matches!(
        token,
        "abnormal"
            | "absent"
            | "altered"
            | "blurred"
            | "decreased"
            | "disturbed"
            | "erratic"
            | "frequent"
            | "heavy"
            | "heavier"
            | "impaired"
            | "increased"
            | "infrequent"
            | "irregular"
            | "light"
            | "low"
            | "missed"
            | "painful"
            | "poor"
            | "prolonged"
            | "reduced"
            | "scanty"
            | "variable"
    )
}

fn likely_clinical_noun_phrase(tokens: &[String]) -> bool {
    if tokens.is_empty() || tokens.len() > 3 {
        return false;
    }

    tokens.iter().all(|token| clinical_noun_phrase_token(token))
        && tokens
            .last()
            .map(|token| clinical_noun_head(token))
            .unwrap_or(false)
}

fn clinical_noun_phrase_token(token: &str) -> bool {
    matches!(
        token,
        "appetite"
            | "balance"
            | "bleeding"
            | "bowel"
            | "concentration"
            | "cycle"
            | "flow"
            | "gait"
            | "hearing"
            | "memory"
            | "menstrual"
            | "menses"
            | "menstruation"
            | "mood"
            | "period"
            | "periods"
            | "sleep"
            | "stool"
            | "urinary"
            | "urination"
            | "urine"
            | "vision"
            | "weight"
    )
}

fn clinical_noun_head(token: &str) -> bool {
    matches!(
        token,
        "appetite"
            | "balance"
            | "bleeding"
            | "concentration"
            | "cycle"
            | "flow"
            | "gait"
            | "hearing"
            | "memory"
            | "menses"
            | "menstruation"
            | "mood"
            | "period"
            | "periods"
            | "sleep"
            | "stool"
            | "urination"
            | "urine"
            | "vision"
            | "weight"
    )
}

fn reorderable_site_descriptor(token: &str) -> bool {
    matches!(
        token,
        "bulging" | "enlarged" | "injected" | "red" | "swollen" | "tender" | "warm"
    )
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

fn site_head_reordered_pattern_tokens(pattern: &str) -> Option<Vec<String>> {
    let tokens = pattern
        .split(' ')
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if tokens.len() < 2 || tokens.len() > 5 {
        return None;
    }

    let head = tokens.last()?;
    if !reorderable_site_head(head) || !likely_body_site_tokens(&tokens[..tokens.len() - 1]) {
        return None;
    }

    let mut reordered = Vec::with_capacity(tokens.len());
    reordered.push(head.clone());
    reordered.extend(tokens[..tokens.len() - 1].iter().cloned());
    Some(reordered)
}

fn reorderable_site_head(token: &str) -> bool {
    matches!(
        token,
        "pain"
            | "ache"
            | "discomfort"
            | "tenderness"
            | "swelling"
            | "weakness"
            | "numbness"
            | "lump"
            | "mass"
            | "discharge"
            | "bleeding"
    )
}

fn likely_body_site_tokens(tokens: &[String]) -> bool {
    if tokens.is_empty() || tokens.len() > 4 {
        return false;
    }

    let mut has_site = false;
    for token in tokens {
        let token = token.as_str();
        if reordered_site_modifier_token(token) {
            continue;
        }
        if reordered_body_site_token(token) {
            has_site = true;
            continue;
        }
        return false;
    }

    has_site
}

fn reordered_site_modifier_token(token: &str) -> bool {
    matches!(
        token,
        "left"
            | "right"
            | "bilateral"
            | "upper"
            | "lower"
            | "central"
            | "lateral"
            | "medial"
            | "anterior"
            | "posterior"
            | "inner"
            | "outer"
    )
}

fn reordered_body_site_token(token: &str) -> bool {
    matches!(
        token,
        "abdomen"
            | "abdominal"
            | "ankle"
            | "arm"
            | "back"
            | "bladder"
            | "bowel"
            | "breast"
            | "calf"
            | "chest"
            | "ear"
            | "eye"
            | "eyelid"
            | "eyelids"
            | "face"
            | "facial"
            | "feet"
            | "foot"
            | "fossa"
            | "groin"
            | "hand"
            | "head"
            | "heel"
            | "hip"
            | "iliac"
            | "jaw"
            | "knee"
            | "leg"
            | "lid"
            | "lids"
            | "limb"
            | "lumbar"
            | "malleoli"
            | "malleolus"
            | "membrane"
            | "neck"
            | "nipple"
            | "pelvic"
            | "pelvis"
            | "perianal"
            | "quadrant"
            | "sacral"
            | "shin"
            | "shoulder"
            | "spinal"
            | "spine"
            | "testicular"
            | "testis"
            | "thigh"
            | "thoracic"
            | "throat"
            | "toe"
            | "tongue"
            | "tonsil"
            | "tonsils"
            | "tympanic"
            | "umbilical"
            | "urethral"
            | "urinary"
            | "vaginal"
            | "vulval"
            | "vulvar"
            | "wrist"
    )
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
    original_text: &str,
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
        if flexible_gap_contains_context_cue(&tokens[end_index + 1..found_index]) {
            return None;
        }
        if has_hard_boundary_between(
            original_text,
            tokens[end_index].original_end,
            tokens[found_index].original_start,
        ) {
            return None;
        }
        extra_tokens += found_index.saturating_sub(search_from);
        if extra_tokens > MAX_EXTRA_TOKENS {
            return None;
        }

        search_from = found_index + 1;
        end_index = found_index;
    }

    Some((tokens[start_index].start, tokens[end_index].end))
}

fn find_body_site_then_head_match(
    original_text: &str,
    tokens: &[NormalizedToken<'_>],
    start_index: usize,
    pattern_tokens: &[String],
) -> Option<(usize, usize)> {
    const MAX_EXTRA_TOKENS: usize = 6;

    if pattern_tokens.len() < 2 || !token_matches(&pattern_tokens[0], tokens.get(start_index)?.text)
    {
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
        if flexible_gap_contains_context_cue(&tokens[end_index + 1..found_index]) {
            return None;
        }
        if has_hard_boundary_between(
            original_text,
            tokens[end_index].original_end,
            tokens[found_index].original_start,
        ) {
            return None;
        }
        extra_tokens += found_index.saturating_sub(search_from);
        if extra_tokens > MAX_EXTRA_TOKENS {
            return None;
        }

        search_from = found_index + 1;
        end_index = found_index;
    }

    Some((tokens[start_index].start, tokens[end_index].end))
}

fn find_clinical_descriptor_final_match(
    original_text: &str,
    tokens: &[NormalizedToken<'_>],
    start_index: usize,
    pattern_tokens: &[String],
) -> Option<(usize, usize)> {
    const MAX_EXTRA_TOKENS: usize = 2;

    if pattern_tokens.len() < 2 || !token_matches(&pattern_tokens[0], tokens.get(start_index)?.text)
    {
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
            if !clinical_descriptor_final_gap_token(candidate_token.text) {
                return None;
            }
        }

        let found_index = found_index?;
        if flexible_gap_contains_context_cue(&tokens[end_index + 1..found_index]) {
            return None;
        }
        if has_hard_boundary_between(
            original_text,
            tokens[end_index].original_end,
            tokens[found_index].original_start,
        ) {
            return None;
        }
        extra_tokens += found_index.saturating_sub(search_from);
        if extra_tokens > MAX_EXTRA_TOKENS {
            return None;
        }

        search_from = found_index + 1;
        end_index = found_index;
    }

    Some((tokens[start_index].start, tokens[end_index].end))
}

fn clinical_descriptor_final_gap_token(token: &str) -> bool {
    matches!(
        token,
        "a" | "an" | "are" | "bit" | "is" | "quite" | "really" | "still" | "the" | "very"
    )
}

fn flexible_gap_contains_context_cue(tokens: &[NormalizedToken<'_>]) -> bool {
    tokens.iter().any(|token| {
        matches!(
            token.text,
            "no" | "not"
                | "without"
                | "nil"
                | "negative"
                | "possible"
                | "possibly"
                | "probable"
                | "suspected"
                | "query"
                | "queried"
        )
    })
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

fn find_site_head_reordered_match(
    original_text: &str,
    tokens: &[NormalizedToken<'_>],
    start_index: usize,
    pattern_tokens: &[String],
) -> Option<(usize, usize)> {
    const MAX_EXTRA_TOKENS: usize = 4;

    if pattern_tokens.len() < 2 || !token_matches(&pattern_tokens[0], tokens.get(start_index)?.text)
    {
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
            if !reordered_site_gap_token(candidate_token.text) {
                return None;
            }
        }

        let found_index = found_index?;
        if has_hard_boundary_between(
            original_text,
            tokens[end_index].original_end,
            tokens[found_index].original_start,
        ) {
            return None;
        }
        extra_tokens += found_index.saturating_sub(search_from);
        if extra_tokens > MAX_EXTRA_TOKENS {
            return None;
        }

        search_from = found_index + 1;
        end_index = found_index;
    }

    Some((tokens[start_index].start, tokens[end_index].end))
}

fn reordered_site_gap_token(token: &str) -> bool {
    matches!(
        token,
        "a" | "an"
            | "the"
            | "in"
            | "of"
            | "on"
            | "to"
            | "into"
            | "from"
            | "around"
            | "down"
            | "over"
            | "both"
            | "bilateral"
            | "left"
            | "right"
            | "l"
            | "r"
            | "upper"
            | "lower"
            | "inner"
            | "outer"
            | "tip"
    )
}

fn has_hard_boundary_between(original_text: &str, start: usize, end: usize) -> bool {
    if start >= end || end > original_text.len() {
        return false;
    }
    original_text[start..end]
        .chars()
        .any(|ch| matches!(ch, '.' | '!' | '?' | ';' | '\n' | '\r'))
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
    if pattern_token == text_token {
        return true;
    }

    let pattern_singular = singular_token(pattern_token);
    let text_singular = singular_token(text_token);
    if pattern_singular == text_singular
        && (pattern_singular != pattern_token || text_singular != text_token)
    {
        return true;
    }

    // Verb morphology is intentionally directional: the clinician text must
    // carry the inflectional evidence. This lets "vomited" match a terminology
    // term "vomiting", but stops a plain base verb such as "hear" from matching
    // a gerund noun/adjective such as "hearing".
    let Some(text_root) = inflection_root(text_token) else {
        return false;
    };
    if text_root == pattern_token || text_root == pattern_singular {
        return true;
    }
    inflection_root(pattern_token)
        .map(|pattern_root| pattern_root == text_root)
        .unwrap_or(false)
}

fn singular_token(token: &str) -> String {
    if token.len() <= 3 {
        return token.to_string();
    }
    if let Some(irregular) = irregular_singular(token) {
        return irregular.to_string();
    }
    if token.ends_with("ss") || token.ends_with("us") || token.ends_with("is") {
        return token.to_string();
    }
    if let Some(stem) = token.strip_suffix("ies") {
        return format!("{stem}y");
    }
    if let Some(stem) = token.strip_suffix("ves") {
        return format!("{stem}f");
    }
    if token.ends_with("ches") && !token.ends_with("aches") {
        return token.trim_end_matches("es").to_string();
    }
    for suffix in ["shes", "xes", "zes", "sses"] {
        if token.ends_with(suffix) {
            return token.trim_end_matches("es").to_string();
        }
    }
    if token.ends_with("ses") {
        return token.trim_end_matches('s').to_string();
    }
    if let Some(stem) = token.strip_suffix('s') {
        return stem.to_string();
    }

    token.to_string()
}

fn irregular_singular(token: &str) -> Option<&'static str> {
    match token {
        "children" => Some("child"),
        "feet" => Some("foot"),
        "teeth" => Some("tooth"),
        "men" => Some("man"),
        "women" => Some("woman"),
        "people" => Some("person"),
        "criteria" => Some("criterion"),
        "phenomena" => Some("phenomenon"),
        "indices" => Some("index"),
        "appendices" => Some("appendix"),
        _ => None,
    }
}

fn inflection_root(token: &str) -> Option<String> {
    if token.len() <= 4 {
        return None;
    }

    if let Some(stem) = token.strip_suffix("ied") {
        if stem.len() >= 3 {
            return Some(format!("{stem}y"));
        }
    }

    if let Some(stem) = token.strip_suffix("ing") {
        return normalize_inflection_stem(stem);
    }

    if let Some(stem) = token.strip_suffix("ed") {
        return normalize_inflection_stem(stem);
    }

    None
}

fn normalize_inflection_stem(stem: &str) -> Option<String> {
    if stem.len() < 4 {
        return None;
    }

    let undoubled = undouble_final_consonant(stem);
    Some(undoubled.to_string())
}

fn undouble_final_consonant(stem: &str) -> &str {
    let mut chars = stem.char_indices().rev();
    let Some((last_idx, last)) = chars.next() else {
        return stem;
    };
    let Some((previous_idx, previous)) = chars.next() else {
        return stem;
    };
    if last == previous && is_ascii_consonant(last) {
        &stem[..last_idx.max(previous_idx + previous.len_utf8())]
    } else {
        stem
    }
}

fn is_ascii_consonant(ch: char) -> bool {
    ch.is_ascii_alphabetic() && !matches!(ch, 'a' | 'e' | 'i' | 'o' | 'u')
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

    fn artefact_with(concepts: Vec<ConceptEntry>) -> TerminologyArtefact {
        TerminologyArtefact {
            schema_version: 1,
            terminology_version: "test".to_string(),
            source_release: "test".to_string(),
            refset_id: "test-refset".to_string(),
            generated_at_utc: "test".to_string(),
            concepts,
            artefact_hash: String::new(),
        }
    }

    fn concept(concept_id: &str, preferred_term: &str, variants: &[&str]) -> ConceptEntry {
        concept_with_source(concept_id, preferred_term, variants, "fixture")
    }

    fn concept_with_source(
        concept_id: &str,
        preferred_term: &str,
        variants: &[&str],
        source: &str,
    ) -> ConceptEntry {
        ConceptEntry {
            concept_id: concept_id.to_string(),
            active: true,
            preferred_term: preferred_term.to_string(),
            descriptions: vec![],
            variants: variants
                .iter()
                .map(|text| TermVariant {
                    text: text.to_string(),
                    source: source.to_string(),
                    description_id: None,
                    allow_ambiguous: false,
                    requires_numeric_value: false,
                })
                .collect(),
        }
    }

    fn raw_match(concept_id: &str, normalized_match: &str) -> RawMatch {
        raw_match_with_span(concept_id, normalized_match, 10, 15)
    }

    fn raw_match_with_span(
        concept_id: &str,
        normalized_match: &str,
        span_start: usize,
        span_end: usize,
    ) -> RawMatch {
        RawMatch {
            concept_id: concept_id.to_string(),
            preferred_term: format!("Concept {concept_id}"),
            field: SoapField::History,
            span_start,
            span_end,
            matched_text: "SOBOE".to_string(),
            normalized_match: normalized_match.to_string(),
            pattern_source: "fixture".to_string(),
            value: None,
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
    fn does_not_match_phrases_across_sentence_boundaries() {
        let matcher = TerminologyMatcher::new(&artefact_with(vec![concept(
            "1000000110",
            "Alpha beta",
            &["alpha beta"],
        )]))
        .unwrap();

        let matches = matcher.find_in_field(SoapField::History, "alpha. beta", false);

        assert!(matches.is_empty());
    }

    #[test]
    fn flexible_matches_do_not_cross_sentence_boundaries() {
        let matcher = TerminologyMatcher::new(&artefact_with(vec![concept(
            "1000000111",
            "Alpha of beta gamma",
            &["alpha of beta gamma"],
        )]))
        .unwrap();

        let matches = matcher.find_in_field(SoapField::History, "alpha of beta. gamma", false);

        assert!(matches.is_empty());
    }

    #[test]
    fn reordered_site_head_matches_pain_and_lump_mentions() {
        let matcher = TerminologyMatcher::new(&artefact_with(vec![
            concept("22253000", "Pain", &["pain"]),
            concept("300848003", "Mass of body structure", &["lump"]),
            concept("1000000201", "Pain in calf", &["calf pain"]),
            concept("1000000202", "Breast lump", &["breast lump"]),
        ]))
        .unwrap();

        let calf = matcher.find_in_field(
            SoapField::History,
            "cramping pain both calves on walking",
            false,
        );
        assert_eq!(calf.len(), 1);
        assert_eq!(calf[0].concept_id, "1000000201");
        assert_eq!(calf[0].matched_text, "pain both calves");

        let breast = matcher.find_in_field(SoapField::History, "lump R breast", false);
        assert_eq!(breast.len(), 1);
        assert_eq!(breast[0].concept_id, "1000000202");
        assert_eq!(breast[0].matched_text, "lump R breast");
    }

    #[test]
    fn reordered_site_head_rejects_unrelated_gap_words() {
        let matcher = TerminologyMatcher::new(&artefact_with(vec![concept(
            "1000000203",
            "Pain in calf",
            &["calf pain"],
        )]))
        .unwrap();

        let matches = matcher.find_in_field(
            SoapField::History,
            "pain after fatty meals, calves fine",
            false,
        );

        assert!(matches.is_empty());
    }

    #[test]
    fn descriptor_final_matches_short_clinical_shorthand() {
        let matcher = TerminologyMatcher::new(&artefact_with(vec![
            concept("386692008", "Menorrhagia", &["heavy periods"]),
            concept("1000000205", "Poor sleep", &["poor sleep"]),
        ]))
        .unwrap();

        let periods = matcher.find_in_field(SoapField::History, "Periods heavy", false);
        assert_eq!(periods.len(), 1);
        assert_eq!(periods[0].concept_id, "386692008");
        assert_eq!(periods[0].matched_text, "Periods heavy");
        assert_eq!(periods[0].normalized_match, "heavy periods");
        assert!(periods[0]
            .pattern_source
            .ends_with(":clinical-descriptor-final"));

        let sleep = matcher.find_in_field(SoapField::History, "Sleep is still poor", false);
        assert_eq!(sleep.len(), 1);
        assert_eq!(sleep[0].concept_id, "1000000205");
        assert_eq!(sleep[0].matched_text, "Sleep is still poor");
    }

    #[test]
    fn descriptor_final_rejects_nonclinical_noun_phrases() {
        let matcher = TerminologyMatcher::new(&artefact_with(vec![concept(
            "1000000206",
            "Heavy feet",
            &["heavy feet"],
        )]))
        .unwrap();

        let matches = matcher.find_in_field(SoapField::History, "Feet heavy", false);

        assert!(matches.is_empty());
    }

    #[test]
    fn body_site_head_match_does_not_cross_negation_cues() {
        let matcher = TerminologyMatcher::new(&artefact_with(vec![concept(
            "1000000204",
            "Perforation of tympanic membrane",
            &["perforation of tympanic membrane"],
        )]))
        .unwrap();

        let matches = matcher.find_in_field(
            SoapField::Objective,
            "TM red and bulging, no perforation",
            false,
        );

        assert!(matches.is_empty());
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
    fn matches_regular_plural_mentions_without_term_specific_aliases() {
        let matcher = TerminologyMatcher::new(&artefact_with(vec![concept(
            "1000000100",
            "Target symptom",
            &["target symptom"],
        )]))
        .unwrap();

        let matches = matcher.find_in_field(SoapField::History, "recurrent target symptoms", false);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].concept_id, "1000000100");
        assert_eq!(matches[0].matched_text, "target symptoms");
        assert!(matches[0].pattern_source.ends_with(":morphology"));
    }

    #[test]
    fn matches_past_tense_mentions_against_gerund_terms() {
        let matcher = TerminologyMatcher::new(&artefact_with(vec![concept(
            "1000000101",
            "Targeting",
            &["targeting"],
        )]))
        .unwrap();

        let matches = matcher.find_in_field(SoapField::History, "targeted twice today", false);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].concept_id, "1000000101");
        assert_eq!(matches[0].matched_text, "targeted");
    }

    #[test]
    fn does_not_match_plain_base_verbs_to_gerund_terms() {
        let matcher = TerminologyMatcher::new(&artefact_with(vec![concept(
            "1000000102",
            "Hearing",
            &["hearing"],
        )]))
        .unwrap();

        let matches = matcher.find_in_field(SoapField::History, "can hear clearly", false);

        assert!(matches.is_empty());
    }

    #[test]
    fn prefers_longest_morphological_phrase() {
        let matcher = TerminologyMatcher::new(&artefact_with(vec![
            concept("1000000103", "Target symptom", &["target symptom"]),
            concept(
                "1000000104",
                "Severe target symptom",
                &["severe target symptom"],
            ),
        ]))
        .unwrap();

        let matches =
            matcher.find_in_field(SoapField::History, "severe target symptoms today", false);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].concept_id, "1000000104");
        assert_eq!(matches[0].matched_text, "severe target symptoms");
    }

    #[test]
    fn blocks_morphological_forms_that_collapse_to_multiple_concepts() {
        let matcher = TerminologyMatcher::new(&artefact_with(vec![
            concept("1000000105", "Target", &["target"]),
            concept("1000000106", "Targeting", &["targeting"]),
        ]))
        .unwrap();

        let matches = matcher.find_in_field(SoapField::History, "targeted today", false);

        assert!(matches.is_empty());
    }

    #[test]
    fn official_term_source_beats_ambiguous_derived_variant() {
        let matcher = TerminologyMatcher::new(&artefact_with(vec![
            concept_with_source(
                "1000000107",
                "First concept",
                &["shared term"],
                "openehr-description-synonym",
            ),
            concept_with_source(
                "1000000108",
                "Second concept",
                &["shared term"],
                "openehr-description-clinical-phrase-variant",
            ),
        ]))
        .unwrap();

        let matches = matcher.find_in_field(SoapField::History, "shared term", false);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].concept_id, "1000000107");
    }

    #[test]
    fn removes_same_span_less_specific_qualified_matches() {
        let mut matches = vec![
            raw_match("100", "shortness of breath"),
            raw_match("101", "shortness of breath on exertion"),
        ];

        remove_subsumed_overlapping_matches(&mut matches);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].concept_id, "101");
    }

    #[test]
    fn removes_nested_less_specific_qualified_matches() {
        let mut matches = vec![
            raw_match_with_span("100", "painful", 10, 17),
            raw_match_with_span("101", "painful arc", 10, 21),
        ];

        remove_subsumed_overlapping_matches(&mut matches);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].concept_id, "101");
    }

    #[test]
    fn keeps_same_span_coordinated_components() {
        let mut matches = vec![
            raw_match("100", "nausea"),
            raw_match("101", "vomiting"),
            raw_match("102", "nausea and vomiting"),
        ];

        remove_subsumed_overlapping_matches(&mut matches);

        assert_eq!(matches.len(), 3);
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
