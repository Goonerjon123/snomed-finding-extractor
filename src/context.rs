use crate::model::{AssertionStatus, SoapField};
use crate::normalization::normalize_term;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertionDecision {
    pub accepted: bool,
    pub assertion: AssertionStatus,
    pub rule_ids: Vec<String>,
    pub explanation: String,
}

#[derive(Debug, Clone)]
struct RuleHit {
    assertion: AssertionStatus,
    rule_id: &'static str,
    explanation: &'static str,
    priority: u8,
}

pub fn classify_assertion(
    field: SoapField,
    field_text: &str,
    span_start: usize,
    span_end: usize,
) -> AssertionDecision {
    let (sentence_start, sentence_end) = sentence_bounds(field_text, span_start, span_end);
    let sentence = &field_text[sentence_start..sentence_end];
    let prefix = &field_text[sentence_start..span_start];
    let immediate_prefix = &field_text[sentence_start..span_start];

    let sentence_norm = padded_norm(sentence);
    let prefix_norm = padded_norm(prefix);
    let contrast_idx = last_phrase_index(&prefix_norm, &["but", "however", "although", "though"]);
    let mut hits = Vec::new();

    if field == SoapField::Plan {
        hits.push(RuleHit {
            assertion: AssertionStatus::Planned,
            rule_id: "PLAN_FIELD_REVIEW_ONLY",
            explanation: "plan field mentions are review-only unless a future ruleset explicitly permits them",
            priority: 40,
        });
    }

    if contains_any(
        &sentence_norm,
        &[
            "family history",
            "fhx",
            "father",
            "mother",
            "brother",
            "sister",
            "son",
            "daughter",
            "parent",
            "grandmother",
            "grandfather",
        ],
    ) {
        hits.push(RuleHit {
            assertion: AssertionStatus::FamilyHistory,
            rule_id: "CTX_FAMILY_HISTORY",
            explanation: "the mention appears in family-history or relative context",
            priority: 10,
        });
    }

    if contains_any(
        &sentence_norm,
        &["wife", "husband", "partner", "carer", "friend"],
    ) {
        hits.push(RuleHit {
            assertion: AssertionStatus::NonPatient,
            rule_id: "CTX_NON_PATIENT_EXPERIENCER",
            explanation: "the mention appears to refer to someone other than the patient",
            priority: 11,
        });
    }

    if has_active_scope_phrase(
        &prefix_norm,
        &[
            "no",
            "not",
            "denies",
            "denied",
            "deny",
            "without",
            "negative for",
            "free of",
            "absence of",
            "never had",
            "nil",
        ],
        contrast_idx,
    ) {
        hits.push(RuleHit {
            assertion: AssertionStatus::Negated,
            rule_id: "CTX_NEGATED_PRECEDING",
            explanation: "a negation cue scopes over the mention",
            priority: 20,
        });
    }

    if has_active_scope_phrase(
        &prefix_norm,
        &[
            "possible",
            "possibly",
            "probable",
            "suspected",
            "suspicion of",
            "query",
            "rule out",
            "r o",
            "differential",
            "consider",
            "concern about",
        ],
        contrast_idx,
    ) || immediate_prefix.trim_end().ends_with('?')
    {
        hits.push(RuleHit {
            assertion: AssertionStatus::Uncertain,
            rule_id: "CTX_UNCERTAIN_OR_QUERY",
            explanation: "an uncertainty or query cue scopes over the mention",
            priority: 30,
        });
    }

    if has_active_scope_phrase(
        &prefix_norm,
        &[
            "history of",
            "h o",
            "past history of",
            "past medical history",
            "pmh",
            "previous",
            "previously",
            "resolved",
            "resolved history of",
            "old",
        ],
        contrast_idx,
    ) {
        hits.push(RuleHit {
            assertion: AssertionStatus::HistoricalOrResolved,
            rule_id: "CTX_HISTORICAL_OR_RESOLVED",
            explanation: "the mention is framed as historical, previous, or resolved",
            priority: 50,
        });
    }

    if has_active_scope_phrase(
        &prefix_norm,
        &[
            "if", "when", "unless", "should", "would", "could", "risk of",
        ],
        contrast_idx,
    ) {
        hits.push(RuleHit {
            assertion: AssertionStatus::Conditional,
            rule_id: "CTX_CONDITIONAL_OR_HYPOTHETICAL",
            explanation: "the mention is conditional or hypothetical",
            priority: 60,
        });
    }

    if field == SoapField::Plan
        || has_active_scope_phrase(
            &prefix_norm,
            &[
                "refer for",
                "test for",
                "screen for",
                "monitor for",
                "arrange",
                "plan to",
            ],
            contrast_idx,
        )
    {
        hits.push(RuleHit {
            assertion: AssertionStatus::Planned,
            rule_id: "CTX_PLANNED_ACTION",
            explanation: "the mention is part of a planned action rather than an asserted finding",
            priority: 41,
        });
    }

    if hits.is_empty() {
        return AssertionDecision {
            accepted: true,
            assertion: AssertionStatus::Affirmed,
            rule_ids: vec!["ASSERT_AFFIRMED_PATIENT_FINDING".to_string()],
            explanation: format!(
                "Accepted as an affirmed patient finding in the {} field; no suppression rule fired.",
                field.as_str()
            ),
        };
    }

    hits.sort_by_key(|hit| hit.priority);
    hits.dedup_by_key(|hit| hit.rule_id);
    let primary = hits[0].assertion;
    let rule_ids = hits
        .iter()
        .map(|hit| hit.rule_id.to_string())
        .collect::<Vec<_>>();
    let explanations = hits
        .iter()
        .map(|hit| hit.explanation)
        .collect::<Vec<_>>()
        .join("; ");

    AssertionDecision {
        accepted: false,
        assertion: primary,
        rule_ids,
        explanation: format!("Suppressed: {explanations}."),
    }
}

fn sentence_bounds(text: &str, span_start: usize, span_end: usize) -> (usize, usize) {
    let start = text[..span_start]
        .rfind(['.', '!', ';', '\n', '\r'])
        .map(|idx| idx + 1)
        .unwrap_or(0);
    let end = text[span_end..]
        .find(['.', '!', '?', ';', '\n', '\r'])
        .map(|idx| span_end + idx)
        .unwrap_or(text.len());
    (start, end)
}

fn padded_norm(value: &str) -> String {
    let normalized = normalize_term(value);
    if normalized.is_empty() {
        " ".to_string()
    } else {
        format!(" {normalized} ")
    }
}

fn contains_any(haystack_norm: &str, phrases: &[&str]) -> bool {
    phrases
        .iter()
        .any(|phrase| haystack_norm.contains(&format!(" {} ", normalize_term(phrase))))
}

fn last_phrase_index(haystack_norm: &str, phrases: &[&str]) -> Option<usize> {
    phrases
        .iter()
        .filter_map(|phrase| haystack_norm.rfind(&format!(" {} ", normalize_term(phrase))))
        .max()
}

fn has_active_scope_phrase(
    haystack_norm: &str,
    phrases: &[&str],
    contrast_idx: Option<usize>,
) -> bool {
    let Some(phrase_idx) = last_phrase_index(haystack_norm, phrases) else {
        return false;
    };

    contrast_idx.map(|idx| phrase_idx > idx).unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suppresses_negated_mentions() {
        let text = "No chest pain today";
        let start = text.find("chest pain").unwrap();
        let decision =
            classify_assertion(SoapField::History, text, start, start + "chest pain".len());

        assert!(!decision.accepted);
        assert_eq!(decision.assertion, AssertionStatus::Negated);
    }

    #[test]
    fn contrast_ends_negation_scope() {
        let text = "Denies chest pain but has cough";
        let start = text.find("cough").unwrap();
        let decision = classify_assertion(SoapField::History, text, start, start + "cough".len());

        assert!(decision.accepted);
    }

    #[test]
    fn suppresses_plan_field_by_default() {
        let text = "Screen for depression";
        let start = text.find("depression").unwrap();
        let decision = classify_assertion(SoapField::Plan, text, start, start + "depression".len());

        assert!(!decision.accepted);
        assert_eq!(decision.assertion, AssertionStatus::Planned);
    }
}
