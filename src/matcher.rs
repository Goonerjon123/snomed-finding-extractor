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
}

#[derive(Debug, Clone)]
struct PatternCandidate {
    concept_id: String,
    preferred_term: String,
    pattern: String,
    source: String,
}

#[derive(Debug, Clone)]
pub struct TerminologyMatcher {
    automaton: AhoCorasick,
    patterns: Vec<PatternMeta>,
}

impl TerminologyMatcher {
    pub fn new(artefact: &TerminologyArtefact) -> Result<Self> {
        artefact.validate_runtime_terms()?;

        let mut pattern_strings = Vec::new();
        let mut patterns = Vec::new();
        let mut candidates = Vec::new();
        let mut concepts_by_term: HashMap<String, HashSet<String>> = HashMap::new();
        let mut seen = HashSet::new();

        for concept in artefact.concepts.iter().filter(|concept| concept.active) {
            for variant in concept.runtime_variants() {
                let normalized = normalize_term(&variant.text);
                if normalized.is_empty()
                    || is_blocked_common_term(&normalized, variant.allow_ambiguous)
                {
                    continue;
                }

                concepts_by_term
                    .entry(normalized.clone())
                    .or_default()
                    .insert(concept.concept_id.clone());
                candidates.push(PatternCandidate {
                    concept_id: concept.concept_id.clone(),
                    preferred_term: concept.preferred_term.clone(),
                    pattern: normalized,
                    source: variant.source,
                });
            }
        }

        for candidate in candidates {
            let is_ambiguous = concepts_by_term
                .get(&candidate.pattern)
                .map(|concepts| concepts.len() > 1)
                .unwrap_or(false);
            if is_ambiguous {
                continue;
            }

            let key = (candidate.concept_id.as_str(), candidate.pattern.as_str());
            if !seen.insert((key.0.to_string(), key.1.to_string())) {
                continue;
            }

            pattern_strings.push(candidate.pattern.clone());
            patterns.push(PatternMeta {
                concept_id: candidate.concept_id,
                preferred_term: candidate.preferred_term,
                pattern: candidate.pattern,
                source: candidate.source,
            });
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

        matches
    }
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
