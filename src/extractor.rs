use crate::context::classify_assertion;
use crate::error::{ExtractorError, Result};
use crate::matcher::{RawMatch, TerminologyMatcher};
use crate::model::{
    ExtractRequest, ExtractResponse, FindingMatch, ObservableExtractRequest, SoapField,
    SuppressedMatch,
};
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

    pub fn extract(&self, request: ExtractRequest) -> Result<ExtractResponse> {
        self.extract_with_kind(request, ExtractionKind::Finding)
    }

    pub fn extract_observables(
        &self,
        request: ObservableExtractRequest,
    ) -> Result<ExtractResponse> {
        self.extract_with_kind(request.into(), ExtractionKind::Observable)
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

            for raw in self.matcher.find_in_field(field, text) {
                let decision = classify_assertion(field, text, raw.span_start, raw.span_end);
                if decision.accepted {
                    matches.push(to_finding_match(
                        raw,
                        accepted_rule_ids(extraction_kind),
                        accepted_explanation(extraction_kind, field),
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
}

fn accepted_rule_ids(extraction_kind: ExtractionKind) -> Vec<String> {
    match extraction_kind {
        ExtractionKind::Finding => vec!["ASSERT_AFFIRMED_PATIENT_FINDING".to_string()],
        ExtractionKind::Observable => vec!["ASSERT_AFFIRMED_PATIENT_OBSERVABLE".to_string()],
    }
}

fn accepted_explanation(extraction_kind: ExtractionKind, field: SoapField) -> String {
    match extraction_kind {
        ExtractionKind::Finding => format!(
            "Accepted as an affirmed patient finding in the {} field; no suppression rule fired.",
            field.as_str()
        ),
        ExtractionKind::Observable => format!(
            "Accepted as an affirmed patient observable entity in the {} field; no suppression rule fired.",
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
        confidence: deterministic_confidence(raw.field),
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

fn deterministic_confidence(field: SoapField) -> f32 {
    match field {
        SoapField::Assessment => 0.97,
        SoapField::Objective => 0.94,
        SoapField::History => 0.92,
        SoapField::Plan => 0.80,
    }
}
