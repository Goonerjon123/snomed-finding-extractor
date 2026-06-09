use crate::error::{ExtractorError, Result};
use crate::model::SoapField;
use crate::normalization::{is_normalized_word_boundary, normalize_term, normalize_with_map};
use crate::terminology::{is_blocked_common_term, TerminologyArtefact};
use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use std::collections::{HashMap, HashSet};

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
}

#[derive(Debug, Clone)]
struct NormalizedToken<'a> {
    text: &'a str,
    start: usize,
    end: usize,
}

#[derive(Debug, Clone)]
pub struct TerminologyMatcher {
    automaton: AhoCorasick,
    patterns: Vec<PatternMeta>,
    flexible_patterns: Vec<FlexiblePatternMeta>,
    flexible_by_first_token: HashMap<String, Vec<usize>>,
}

impl TerminologyMatcher {
    pub fn new(artefact: &TerminologyArtefact) -> Result<Self> {
        artefact.validate_runtime_terms()?;

        let mut pattern_strings = Vec::new();
        let mut patterns = Vec::new();
        let mut candidates = Vec::new();
        let mut concepts_by_term: HashMap<String, HashSet<String>> = HashMap::new();
        let mut numeric_concepts_by_term: HashMap<String, HashSet<String>> = HashMap::new();
        let mut seen = HashSet::new();

        for concept in artefact.concepts.iter().filter(|concept| concept.active) {
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
        for candidate in candidates {
            let is_ambiguous = if candidate.requires_numeric_value {
                numeric_concepts_by_term
                    .get(&candidate.pattern)
                    .map(|concepts| concepts.len() > 1)
                    .unwrap_or(false)
            } else {
                concepts_by_term
                    .get(&candidate.pattern)
                    .map(|concepts| concepts.len() > 1)
                    .unwrap_or(false)
            };
            if is_ambiguous {
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
                    let first_token = tokens[0].clone();
                    let flexible_index = flexible_patterns.len();
                    flexible_patterns.push(FlexiblePatternMeta {
                        concept_id: candidate.concept_id,
                        preferred_term: candidate.preferred_term,
                        pattern: candidate.pattern,
                        source: format!("{}:flexible-body-site", candidate.source),
                        tokens,
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
            }
        }

        if patterns.is_empty() {
            return Err(ExtractorError::EmptyTerminology);
        }

        let automaton = AhoCorasickBuilder::new()
            .match_kind(MatchKind::LeftmostLongest)
            .build(pattern_strings)
            .map_err(|err| ExtractorError::Matcher(err.to_string()))?;

        Ok(Self {
            automaton,
            patterns,
            flexible_patterns,
            flexible_by_first_token,
        })
    }

    pub fn find_in_field(&self, field: SoapField, text: &str) -> Vec<RawMatch> {
        let normalized = normalize_with_map(text);
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
            if meta.requires_numeric_value
                && !has_numeric_value_after(&normalized.text, found.end())
            {
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
            });
        }

        let tokens = normalized_tokens(&normalized.text);
        for (token_index, token) in tokens.iter().enumerate() {
            for key in [token.text.to_string(), singular_token(token.text)] {
                let Some(pattern_indices) = self.flexible_by_first_token.get(&key) else {
                    continue;
                };

                for pattern_index in pattern_indices {
                    let meta = &self.flexible_patterns[*pattern_index];
                    let Some((start, end)) =
                        find_flexible_body_site_match(&tokens, token_index, &meta.tokens)
                    else {
                        continue;
                    };
                    let Some((span_start, span_end)) = normalized.original_range(start, end) else {
                        continue;
                    };

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
                    });
                }
            }
        }

        matches
    }
}

fn has_numeric_value_after(normalized_text: &str, pattern_end: usize) -> bool {
    normalized_text[pattern_end..]
        .trim_start()
        .chars()
        .next()
        .map(|ch| ch.is_ascii_digit() || ch == '-' || ch == '+')
        .unwrap_or(false)
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

fn normalized_tokens(text: &str) -> Vec<NormalizedToken<'_>> {
    let mut tokens = Vec::new();
    let mut token_start = None;
    for (idx, ch) in text.char_indices() {
        if ch == ' ' {
            if let Some(start) = token_start.take() {
                tokens.push(NormalizedToken {
                    text: &text[start..idx],
                    start,
                    end: idx,
                });
            }
        } else if token_start.is_none() {
            token_start = Some(idx);
        }
    }
    if let Some(start) = token_start {
        tokens.push(NormalizedToken {
            text: &text[start..],
            start,
            end: text.len(),
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
        let matches = matcher.find_in_field(SoapField::Assessment, "CHEST-pain present");

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].matched_text, "CHEST-pain");
    }

    #[test]
    fn skips_terms_that_map_to_multiple_concepts() {
        let mut artefact = artefact();
        artefact.concepts.push(ConceptEntry {
            concept_id: "1000000002".to_string(),
            active: true,
            preferred_term: "Different concept".to_string(),
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
            .find_in_field(SoapField::History, "chest pain")
            .is_empty());
        assert_eq!(
            matcher
                .find_in_field(SoapField::History, "unique symptom")
                .len(),
            1
        );
    }
}
