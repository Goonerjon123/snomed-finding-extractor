use crate::context::classify_assertion;
use crate::error::{ExtractorError, Result};
use crate::matcher::{RawMatch, TerminologyMatcher};
use crate::model::{
    AssertionStatus, DiagnosisExtractRequest, ExaminationFindingsExtractRequest, ExtractRequest,
    ExtractResponse, FindingMatch, ObservableExtractRequest, SoapField, SuppressedMatch,
};
use crate::normalization::normalize_clinical_text;
use crate::terminology::TerminologyArtefact;
use crate::{ENGINE_VERSION, RULESET_VERSION};
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct Extractor {
    artefact: TerminologyArtefact,
    matcher: TerminologyMatcher,
}

impl Extractor {
    pub fn new(artefact: TerminologyArtefact) -> Result<Self> {
        let matcher = TerminologyMatcher::new(&artefact)?;
        Ok(Self { artefact, matcher })
    }

    pub fn artefact(&self) -> &TerminologyArtefact {
        &self.artefact
    }

    /// Terms the ambiguity guard removed from this artefact at build time.
    pub fn dropped_ambiguous_terms(&self) -> &[crate::matcher::DroppedTerm] {
        self.matcher.dropped_ambiguous()
    }

    pub fn extract(&self, request: ExtractRequest) -> Result<ExtractResponse> {
        self.extract_with_kind(request, ExtractionKind::Finding)
    }

    pub fn extract_observables(
        &self,
        request: ObservableExtractRequest,
    ) -> Result<ExtractResponse> {
        self.extract_with_kind(request.into(), ExtractionKind::Observable)
    }

    pub fn extract_examination_findings(
        &self,
        request: ExaminationFindingsExtractRequest,
    ) -> Result<ExtractResponse> {
        self.extract_with_kind(request.into(), ExtractionKind::ExaminationFinding)
    }

    pub fn extract_diagnoses(&self, request: DiagnosisExtractRequest) -> Result<ExtractResponse> {
        self.extract_with_kind(request.into(), ExtractionKind::Diagnosis)
    }

    fn extract_with_kind(
        &self,
        request: ExtractRequest,
        extraction_kind: ExtractionKind,
    ) -> Result<ExtractResponse> {
        if let Some(requested_refset) = request.refset_id.as_ref() {
            if requested_refset != &self.artefact.refset_id {
                return Err(ExtractorError::RefsetMismatch {
                    requested: requested_refset.clone(),
                    loaded: self.artefact.refset_id.clone(),
                });
            }
        }

        let started = Instant::now();
        let mut matches = Vec::new();
        let mut suppressed = Vec::new();

        for (field, text) in request.fields() {
            if text.trim().is_empty() {
                continue;
            }

            let capture_values = matches!(extraction_kind, ExtractionKind::Observable);
            let raw_matches = self.matcher.find_in_field(field, text, capture_values);
            let spans = raw_matches
                .iter()
                .map(|raw| (raw.span_start, raw.span_end))
                .collect::<Vec<_>>();

            for (index, raw) in raw_matches.into_iter().enumerate() {
                // Sibling spans let a cue scope across coordinated matches:
                // "no cough or wheeze" suppresses both.
                let siblings = spans
                    .iter()
                    .enumerate()
                    .filter_map(|(other, span)| (other != index).then_some(*span))
                    .collect::<Vec<_>>();
                let decision = semantic_context_decision(&raw, text).unwrap_or_else(|| {
                    classify_assertion(field, text, raw.span_start, raw.span_end, &siblings)
                });
                if decision.accepted {
                    matches.push(to_finding_match(
                        raw,
                        accepted_rule_ids(extraction_kind, &decision),
                        accepted_explanation(extraction_kind, field, &decision),
                    ));
                } else if request.include_suppressed {
                    suppressed.push(to_suppressed_match(
                        raw,
                        decision.assertion,
                        decision.rule_ids,
                        decision.explanation,
                    ));
                }
            }
        }

        matches.sort_by_key(|item| (item.field, item.span_start, item.concept_id.clone()));
        suppressed.sort_by_key(|item| (item.field, item.span_start, item.concept_id.clone()));

        Ok(ExtractResponse {
            note_id: request.note_id,
            matches,
            suppressed,
            terminology_version: self.artefact.terminology_version.clone(),
            engine_version: ENGINE_VERSION.to_string(),
            ruleset_version: RULESET_VERSION.to_string(),
            artefact_hash: self.artefact.artefact_hash.clone(),
            elapsed_micros: started.elapsed().as_micros(),
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum ExtractionKind {
    Finding,
    Observable,
    ExaminationFinding,
    Diagnosis,
}

fn kind_rule_id(extraction_kind: ExtractionKind) -> &'static str {
    match extraction_kind {
        ExtractionKind::Finding => "ASSERT_AFFIRMED_PATIENT_FINDING",
        ExtractionKind::Observable => "ASSERT_AFFIRMED_PATIENT_OBSERVABLE",
        ExtractionKind::ExaminationFinding => "ASSERT_AFFIRMED_PATIENT_EXAMINATION_FINDING",
        ExtractionKind::Diagnosis => "ASSERT_AFFIRMED_PATIENT_DIAGNOSIS",
    }
}

fn semantic_context_decision(
    raw: &RawMatch,
    field_text: &str,
) -> Option<crate::context::AssertionDecision> {
    if raw.concept_id == "75088002"
        && raw.normalized_match == "urgency"
        && !has_urinary_context(raw.field, field_text, raw.span_start, raw.span_end)
    {
        return Some(crate::context::AssertionDecision {
            accepted: false,
            assertion: AssertionStatus::Ambiguous,
            rule_ids: vec!["CTX_AMBIGUOUS_URGENCY_WITHOUT_URINARY_CONTEXT".to_string()],
            explanation:
                "Suppressed: bare urgency is not specific to urinary urgency without urinary context."
                    .to_string(),
        });
    }

    if raw.concept_id == "278017001"
        && is_bare_smell_descriptor(&raw.normalized_match)
        && !has_urinary_context(raw.field, field_text, raw.span_start, raw.span_end)
    {
        return Some(crate::context::AssertionDecision {
            accepted: false,
            assertion: AssertionStatus::Ambiguous,
            rule_ids: vec!["CTX_AMBIGUOUS_URINE_SMELL_WITHOUT_URINARY_CONTEXT".to_string()],
            explanation: "Suppressed: smell descriptor is not specific to malodorous urine without urinary context."
                .to_string(),
        });
    }

    None
}

fn is_bare_smell_descriptor(normalized_match: &str) -> bool {
    matches!(
        normalized_match,
        "strong smelling" | "foul smelling" | "offensive smelling" | "smelly"
    )
}

fn has_urinary_context(field: SoapField, text: &str, span_start: usize, span_end: usize) -> bool {
    let (window_start, window_end) = context_window(text, span_start, span_end, 120);
    let normalized = normalize_clinical_text(&text[window_start..window_end], field).text;
    let tokens = normalized
        .split(' ')
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();

    tokens.iter().any(|token| {
        matches!(
            *token,
            "urine"
                | "urinary"
                | "bladder"
                | "waterworks"
                | "micturition"
                | "micturate"
                | "dysuria"
                | "haematuria"
                | "hematuria"
                | "nocturia"
                | "stream"
                | "flow"
                | "dribbling"
                | "incontinence"
                | "wee"
                | "pee"
        )
    }) || normalized.contains("pass urine")
        || normalized.contains("passing urine")
        || normalized.contains("empty bladder")
}

fn context_window(text: &str, span_start: usize, span_end: usize, radius: usize) -> (usize, usize) {
    let mut start = span_start.saturating_sub(radius);
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }

    let mut end = (span_end + radius).min(text.len());
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }

    (start, end)
}

fn accepted_rule_ids(
    extraction_kind: ExtractionKind,
    decision: &crate::context::AssertionDecision,
) -> Vec<String> {
    decision
        .rule_ids
        .iter()
        .map(|rule_id| {
            if rule_id == "ASSERT_AFFIRMED_PATIENT_FINDING" {
                kind_rule_id(extraction_kind).to_string()
            } else {
                rule_id.clone()
            }
        })
        .collect()
}

fn accepted_explanation(
    extraction_kind: ExtractionKind,
    field: SoapField,
    decision: &crate::context::AssertionDecision,
) -> String {
    if decision
        .rule_ids
        .iter()
        .any(|rule_id| rule_id == "PLAN_COMPLETED_ACTION")
    {
        return decision.explanation.clone();
    }

    kind_explanation(extraction_kind, field)
}

fn kind_explanation(extraction_kind: ExtractionKind, field: SoapField) -> String {
    match extraction_kind {
        ExtractionKind::Finding => format!(
            "Accepted as an affirmed patient finding in the {} field; no suppression rule fired.",
            field.as_str()
        ),
        ExtractionKind::Observable => format!(
            "Accepted as an affirmed patient observable entity in the {} field; no suppression rule fired.",
            field.as_str()
        ),
        ExtractionKind::ExaminationFinding => format!(
            "Accepted as an affirmed patient examination finding in the {} field; no suppression rule fired.",
            field.as_str()
        ),
        ExtractionKind::Diagnosis => format!(
            "Accepted as an affirmed patient diagnosis/disorder in the {} field; no suppression rule fired.",
            field.as_str()
        ),
    }
}

fn to_finding_match(raw: RawMatch, rule_ids: Vec<String>, explanation: String) -> FindingMatch {
    FindingMatch {
        concept_id: raw.concept_id,
        preferred_term: raw.preferred_term,
        field: raw.field,
        span_start: raw.span_start,
        span_end: raw.span_end,
        matched_text: raw.matched_text,
        normalized_match: raw.normalized_match,
        term_source: raw.pattern_source,
        value: raw.value,
        rule_ids,
        explanation,
    }
}

fn to_suppressed_match(
    raw: RawMatch,
    assertion: crate::model::AssertionStatus,
    rule_ids: Vec<String>,
    explanation: String,
) -> SuppressedMatch {
    SuppressedMatch {
        concept_id: raw.concept_id,
        preferred_term: raw.preferred_term,
        field: raw.field,
        span_start: raw.span_start,
        span_end: raw.span_end,
        matched_text: raw.matched_text,
        normalized_match: raw.normalized_match,
        assertion,
        rule_ids,
        explanation,
    }
}
