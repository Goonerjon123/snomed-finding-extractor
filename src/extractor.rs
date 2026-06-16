use crate::context::classify_assertion;
use crate::error::{ExtractorError, Result};
use crate::matcher::{RawMatch, TerminologyMatcher};
use crate::model::{
    AssertionStatus, BodySiteMatch, DiagnosisExtractRequest, ExaminationFindingsExtractRequest,
    ExtractRequest, ExtractResponse, FindingMatch, MeasuredValue, ObservableExtractRequest,
    SoapField, SuppressedMatch,
};
use crate::normalization::{normalize_clinical_text, normalize_term, NormalizedText};
use crate::terminology::TerminologyArtefact;
use crate::{ENGINE_VERSION, RULESET_VERSION};
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct Extractor {
    artefact: TerminologyArtefact,
    matcher: TerminologyMatcher,
    body_site_matcher: Option<TerminologyMatcher>,
}

impl Extractor {
    pub fn new(artefact: TerminologyArtefact) -> Result<Self> {
        let matcher = TerminologyMatcher::new(&artefact)?;
        Ok(Self {
            artefact,
            matcher,
            body_site_matcher: None,
        })
    }

    pub fn new_with_body_sites(
        artefact: TerminologyArtefact,
        body_site_artefact: TerminologyArtefact,
    ) -> Result<Self> {
        let matcher = TerminologyMatcher::new(&artefact)?;
        let body_site_matcher = TerminologyMatcher::new(&body_site_artefact)?;
        Ok(Self {
            artefact,
            matcher,
            body_site_matcher: Some(body_site_matcher),
        })
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
            let body_site_matches = if matches!(extraction_kind, ExtractionKind::Finding) {
                self.body_site_matcher
                    .as_ref()
                    .map(|matcher| matcher.find_in_field(field, text, false))
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
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
                let decision = semantic_context_decision(&raw, text, extraction_kind)
                    .unwrap_or_else(|| {
                        classify_assertion(field, text, raw.span_start, raw.span_end, &siblings)
                    });
                if decision.accepted {
                    let body_site = if matches!(extraction_kind, ExtractionKind::Finding) {
                        self.body_site_for_match(&raw, text, &body_site_matches)
                    } else {
                        None
                    };
                    matches.push(to_finding_match(
                        raw,
                        accepted_rule_ids(extraction_kind, &decision),
                        accepted_explanation(extraction_kind, field, &decision),
                        body_site,
                        decision.assertion,
                    ));
                } else if materialize_non_affirmed_match(extraction_kind, decision.assertion) {
                    let explanation =
                        non_affirmed_match_explanation(extraction_kind, field, &decision);
                    matches.push(to_finding_match(
                        raw,
                        decision.rule_ids,
                        explanation,
                        None,
                        decision.assertion,
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

            if matches!(extraction_kind, ExtractionKind::ExaminationFinding) {
                add_normal_examination_matches(field, text, &mut matches);
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

    fn body_site_for_match(
        &self,
        raw: &RawMatch,
        field_text: &str,
        body_site_matches: &[RawMatch],
    ) -> Option<BodySiteMatch> {
        let body_site_matcher = self.body_site_matcher.as_ref()?;
        if symptom_already_implies_body_site(raw, body_site_matcher) {
            return None;
        }

        self.local_body_site_for_match(raw, field_text, body_site_matches)
            .or_else(|| heading_body_site_for_match(raw, field_text, body_site_matches))
    }

    fn local_body_site_for_match(
        &self,
        raw: &RawMatch,
        field_text: &str,
        body_site_matches: &[RawMatch],
    ) -> Option<BodySiteMatch> {
        body_site_matches
            .iter()
            .filter(|site| site.field == raw.field && !generic_body_site(site))
            .filter_map(|site| {
                body_site_association_score(raw, site, field_text)
                    .map(|score| (score, site.span_end - site.span_start, site))
            })
            .min_by(|left, right| {
                left.0
                    .cmp(&right.0)
                    .then_with(|| right.1.cmp(&left.1))
                    .then_with(|| left.2.span_start.cmp(&right.2.span_start))
            })
            .map(|(_, _, site)| body_site_match_from_raw(site))
    }
}

#[derive(Debug, Clone, Copy)]
enum ExtractionKind {
    Finding,
    Observable,
    ExaminationFinding,
    Diagnosis,
}

fn materialize_non_affirmed_match(
    extraction_kind: ExtractionKind,
    assertion: AssertionStatus,
) -> bool {
    matches!(extraction_kind, ExtractionKind::ExaminationFinding)
        && matches!(
            assertion,
            AssertionStatus::Normal | AssertionStatus::Negated | AssertionStatus::Uncertain
        )
}

#[derive(Debug, Clone, Copy)]
struct ExamResultStatus {
    phrase: &'static str,
    assertion: AssertionStatus,
}

struct StructuredExamFeature {
    concept_id: &'static str,
    preferred_term: &'static str,
    subjects: &'static [&'static str],
    statuses_after: &'static [ExamResultStatus],
    statuses_before: &'static [ExamResultStatus],
}

#[derive(Debug, Clone)]
struct ExamToken {
    text: String,
    normalized_start: usize,
    normalized_end: usize,
    orig_start: usize,
    orig_end: usize,
}

struct StandaloneExamFeature {
    concept_id: &'static str,
    preferred_term: &'static str,
    patterns: &'static [&'static str],
    assertion: AssertionStatus,
}

struct NegatedExamSign {
    concept_id: &'static str,
    preferred_term: &'static str,
    heads: &'static [&'static str],
    allow_anatomical_modifiers: bool,
}

const STATUS_NORMAL: &[ExamResultStatus] = &[ExamResultStatus {
    phrase: "normal",
    assertion: AssertionStatus::Normal,
}];

const STATUS_CLEAR: &[ExamResultStatus] = &[ExamResultStatus {
    phrase: "clear",
    assertion: AssertionStatus::Normal,
}];

const STATUS_INTACT_OR_NORMAL: &[ExamResultStatus] = &[
    ExamResultStatus {
        phrase: "intact",
        assertion: AssertionStatus::Normal,
    },
    ExamResultStatus {
        phrase: "normal",
        assertion: AssertionStatus::Normal,
    },
];

const STATUS_SYMMETRICAL_OR_NORMAL: &[ExamResultStatus] = &[
    ExamResultStatus {
        phrase: "symmetrical",
        assertion: AssertionStatus::Normal,
    },
    ExamResultStatus {
        phrase: "symmetric",
        assertion: AssertionStatus::Normal,
    },
    ExamResultStatus {
        phrase: "normal",
        assertion: AssertionStatus::Normal,
    },
    ExamResultStatus {
        phrase: "present",
        assertion: AssertionStatus::Normal,
    },
];

const STATUS_FULL_OR_NORMAL: &[ExamResultStatus] = &[
    ExamResultStatus {
        phrase: "full",
        assertion: AssertionStatus::Normal,
    },
    ExamResultStatus {
        phrase: "normal",
        assertion: AssertionStatus::Normal,
    },
];

const STATUS_SNT_OR_SOFT_NONTENDER: &[ExamResultStatus] = &[
    ExamResultStatus {
        phrase: "snt",
        assertion: AssertionStatus::Normal,
    },
    ExamResultStatus {
        phrase: "soft non tender",
        assertion: AssertionStatus::Normal,
    },
    ExamResultStatus {
        phrase: "soft and non tender",
        assertion: AssertionStatus::Normal,
    },
    ExamResultStatus {
        phrase: "soft nontender",
        assertion: AssertionStatus::Normal,
    },
];

const STATUS_NONTENDER: &[ExamResultStatus] = &[
    ExamResultStatus {
        phrase: "non tender",
        assertion: AssertionStatus::Negated,
    },
    ExamResultStatus {
        phrase: "nontender",
        assertion: AssertionStatus::Negated,
    },
];

const STATUS_PULSATILE: &[ExamResultStatus] = &[ExamResultStatus {
    phrase: "pulsatile",
    assertion: AssertionStatus::Normal,
}];

const STRUCTURED_EXAM_FEATURES: &[StructuredExamFeature] = &[
    StructuredExamFeature {
        concept_id: "271660002",
        preferred_term: "Heart sounds",
        subjects: &["hs", "heart sound", "heart sounds"],
        statuses_after: STATUS_NORMAL,
        statuses_before: &[],
    },
    StructuredExamFeature {
        concept_id: "364060002",
        preferred_term: "Chest auscultation feature",
        subjects: &["chest", "lung", "lungs"],
        statuses_after: STATUS_CLEAR,
        statuses_before: &[],
    },
    StructuredExamFeature {
        concept_id: "271911005",
        preferred_term: "Abdominal examination finding",
        subjects: &["abdomen", "abdominal"],
        statuses_after: STATUS_SNT_OR_SOFT_NONTENDER,
        statuses_before: &[],
    },
    StructuredExamFeature {
        concept_id: "164734008",
        preferred_term: "Fundoscopy normal",
        subjects: &["fundi", "fundus", "fundoscopy"],
        statuses_after: STATUS_NORMAL,
        statuses_before: &[],
    },
    StructuredExamFeature {
        concept_id: "246581004",
        preferred_term: "Peripheral reflex",
        subjects: &["reflex", "reflexes"],
        statuses_after: STATUS_SYMMETRICAL_OR_NORMAL,
        statuses_before: &[],
    },
    StructuredExamFeature {
        concept_id: "363844006",
        preferred_term: "Pattern of coordination",
        subjects: &["coordination"],
        statuses_after: STATUS_NORMAL,
        statuses_before: &[],
    },
    StructuredExamFeature {
        concept_id: "63448001",
        preferred_term: "Gait",
        subjects: &["gait"],
        statuses_after: STATUS_NORMAL,
        statuses_before: &[],
    },
    StructuredExamFeature {
        concept_id: "404980009",
        preferred_term: "Spine - range of movement",
        subjects: &["range of movement"],
        statuses_after: STATUS_FULL_OR_NORMAL,
        statuses_before: STATUS_FULL_OR_NORMAL,
    },
    StructuredExamFeature {
        concept_id: "301399007",
        preferred_term: "Musculoskeletal tenderness",
        subjects: &["temporal artery", "temporal arteries"],
        statuses_after: STATUS_NONTENDER,
        statuses_before: &[],
    },
    StructuredExamFeature {
        concept_id: "422176008",
        preferred_term: "Temporal pulse, function",
        subjects: &["temporal artery", "temporal arteries"],
        statuses_after: STATUS_PULSATILE,
        statuses_before: &[],
    },
];

const STANDALONE_EXAM_FEATURES: &[StandaloneExamFeature] = &[
    StandaloneExamFeature {
        concept_id: "248233002",
        preferred_term: "Mental alertness",
        patterns: &["alert"],
        assertion: AssertionStatus::Normal,
    },
    StandaloneExamFeature {
        concept_id: "43173001",
        preferred_term: "Orientation",
        patterns: &["orientated", "oriented"],
        assertion: AssertionStatus::Normal,
    },
];

const NEGATED_EXAM_SIGNS: &[NegatedExamSign] = &[
    NegatedExamSign {
        concept_id: "423488006",
        preferred_term: "Papilledema - optic disc edema due to raised intracranial pressure",
        heads: &["papilloedema", "papilledema"],
        allow_anatomical_modifiers: false,
    },
    NegatedExamSign {
        concept_id: "301399007",
        preferred_term: "Musculoskeletal tenderness",
        heads: &["tenderness"],
        allow_anatomical_modifiers: true,
    },
    NegatedExamSign {
        concept_id: "56208002",
        preferred_term: "Ulcer",
        heads: &["ulcer", "ulcers"],
        allow_anatomical_modifiers: true,
    },
    NegatedExamSign {
        concept_id: "250087009",
        preferred_term: "Joint deformity",
        heads: &["deformity", "deformities"],
        allow_anatomical_modifiers: true,
    },
    NegatedExamSign {
        concept_id: "24887001",
        preferred_term: "Maceration",
        heads: &["maceration"],
        allow_anatomical_modifiers: true,
    },
    NegatedExamSign {
        concept_id: "3716002",
        preferred_term: "Goiter",
        heads: &["goiter", "goitre"],
        allow_anatomical_modifiers: true,
    },
    NegatedExamSign {
        concept_id: "30746006",
        preferred_term: "Lymphadenopathy",
        heads: &["lymphadenopathy"],
        allow_anatomical_modifiers: true,
    },
    NegatedExamSign {
        concept_id: "247441003",
        preferred_term: "Erythema",
        heads: &["erythema"],
        allow_anatomical_modifiers: true,
    },
];

fn add_normal_examination_matches(
    field: SoapField,
    field_text: &str,
    matches: &mut Vec<FindingMatch>,
) {
    let normalized = normalize_clinical_text(field_text, field);
    let tokens = exam_tokens(&normalized);

    add_structured_feature_status_matches(field, field_text, &normalized, &tokens, matches);
    add_cranial_nerve_exam_matches(field, field_text, &normalized, &tokens, matches);
    add_power_score_exam_matches(field, field_text, &normalized, &tokens, matches);
    add_standalone_exam_matches(field, field_text, &normalized, &tokens, matches);
    add_negated_exam_sign_matches(field, field_text, &normalized, &tokens, matches);
    add_anatomical_tenderness_matches(field, field_text, &normalized, &tokens, matches);
}

fn exam_tokens(normalized: &NormalizedText) -> Vec<ExamToken> {
    let text = normalized.text.as_str();
    let mut tokens = Vec::new();
    let mut cursor = 0;

    while cursor < text.len() {
        let remaining = &text[cursor..];
        let token_start = match remaining.find(|ch| ch != ' ') {
            Some(offset) => cursor + offset,
            None => break,
        };
        let token_end = text[token_start..]
            .find(' ')
            .map(|offset| token_start + offset)
            .unwrap_or(text.len());

        if let Some((orig_start, orig_end)) = normalized.original_range(token_start, token_end) {
            tokens.push(ExamToken {
                text: text[token_start..token_end].to_string(),
                normalized_start: token_start,
                normalized_end: token_end,
                orig_start,
                orig_end,
            });
        }

        cursor = token_end;
    }

    tokens
}

fn add_structured_feature_status_matches(
    field: SoapField,
    field_text: &str,
    normalized: &NormalizedText,
    tokens: &[ExamToken],
    matches: &mut Vec<FindingMatch>,
) {
    for feature in STRUCTURED_EXAM_FEATURES {
        for subject_start in 0..tokens.len() {
            for subject in feature.subjects {
                let Some(subject_end) = match_phrase_at(tokens, subject_start, subject) else {
                    continue;
                };

                if let Some((status_start, status_end, status)) =
                    find_status_after_subject(tokens, subject_end, feature.statuses_after)
                {
                    add_structured_exam_match_from_tokens(
                        field,
                        field_text,
                        normalized,
                        tokens,
                        StructuredExamMatchSpec {
                            start_token: subject_start,
                            end_token: status_end,
                            concept_id: feature.concept_id,
                            preferred_term: feature.preferred_term,
                            assertion: status.assertion,
                            value: None,
                        },
                        matches,
                    );

                    // Keep the status_start binding meaningful for debug/readability.
                    let _ = status_start;
                }

                if let Some((status_start, _status_end, status)) =
                    find_status_before_subject(tokens, subject_start, feature.statuses_before)
                {
                    add_structured_exam_match_from_tokens(
                        field,
                        field_text,
                        normalized,
                        tokens,
                        StructuredExamMatchSpec {
                            start_token: status_start,
                            end_token: subject_end,
                            concept_id: feature.concept_id,
                            preferred_term: feature.preferred_term,
                            assertion: status.assertion,
                            value: None,
                        },
                        matches,
                    );
                }
            }
        }
    }
}

fn add_cranial_nerve_exam_matches(
    field: SoapField,
    field_text: &str,
    normalized: &NormalizedText,
    tokens: &[ExamToken],
    matches: &mut Vec<FindingMatch>,
) {
    for start in 0..tokens.len() {
        let Some(subject_end) = cranial_nerve_subject_end(tokens, start) else {
            continue;
        };
        let Some((_status_start, status_end, status)) =
            find_status_after_subject(tokens, subject_end, STATUS_INTACT_OR_NORMAL)
        else {
            continue;
        };

        add_structured_exam_match_from_tokens(
            field,
            field_text,
            normalized,
            tokens,
            StructuredExamMatchSpec {
                start_token: start,
                end_token: status_end,
                concept_id: "246569003",
                preferred_term: "Function of specific cranial nerves",
                assertion: status.assertion,
                value: None,
            },
            matches,
        );
    }
}

fn add_power_score_exam_matches(
    field: SoapField,
    field_text: &str,
    normalized: &NormalizedText,
    tokens: &[ExamToken],
    matches: &mut Vec<FindingMatch>,
) {
    for start in 0..tokens.len() {
        let Some(subject_end) = match_any_phrase_at(tokens, start, &["power", "muscle power"])
        else {
            continue;
        };
        let Some(value_token) = tokens.get(subject_end) else {
            continue;
        };
        if !looks_like_exam_score(value_token.text.as_str()) {
            continue;
        }

        let assertion = if score_is_normal(value_token.text.as_str()) {
            AssertionStatus::Normal
        } else {
            AssertionStatus::Affirmed
        };
        let value = MeasuredValue {
            text: field_text[value_token.orig_start..value_token.orig_end].to_string(),
            unit: None,
            span_start: value_token.orig_start,
            span_end: value_token.orig_end,
        };

        add_structured_exam_match_from_tokens(
            field,
            field_text,
            normalized,
            tokens,
            StructuredExamMatchSpec {
                start_token: start,
                end_token: subject_end + 1,
                concept_id: "249948009",
                preferred_term: "Grade of muscle power",
                assertion,
                value: Some(value),
            },
            matches,
        );
    }
}

fn add_standalone_exam_matches(
    field: SoapField,
    field_text: &str,
    normalized: &NormalizedText,
    tokens: &[ExamToken],
    matches: &mut Vec<FindingMatch>,
) {
    for feature in STANDALONE_EXAM_FEATURES {
        for start in 0..tokens.len() {
            for pattern in feature.patterns {
                let Some(end) = match_phrase_at(tokens, start, pattern) else {
                    continue;
                };
                add_structured_exam_match_from_tokens(
                    field,
                    field_text,
                    normalized,
                    tokens,
                    StructuredExamMatchSpec {
                        start_token: start,
                        end_token: end,
                        concept_id: feature.concept_id,
                        preferred_term: feature.preferred_term,
                        assertion: feature.assertion,
                        value: None,
                    },
                    matches,
                );
            }
        }
    }
}

fn add_negated_exam_sign_matches(
    field: SoapField,
    field_text: &str,
    normalized: &NormalizedText,
    tokens: &[ExamToken],
    matches: &mut Vec<FindingMatch>,
) {
    for negation_index in 0..tokens.len() {
        if !exam_negation_token(tokens[negation_index].text.as_str()) {
            continue;
        }

        let search_end = (negation_index + 7).min(tokens.len());
        for head_start in negation_index + 1..search_end {
            for sign in NEGATED_EXAM_SIGNS {
                for head in sign.heads {
                    let Some(head_end) = match_phrase_at(tokens, head_start, head) else {
                        continue;
                    };
                    if !negated_exam_sign_gap_is_clear(
                        tokens,
                        negation_index + 1,
                        head_start,
                        sign.allow_anatomical_modifiers,
                    ) {
                        continue;
                    }

                    add_structured_exam_match_from_tokens(
                        field,
                        field_text,
                        normalized,
                        tokens,
                        StructuredExamMatchSpec {
                            start_token: negation_index,
                            end_token: head_end,
                            concept_id: sign.concept_id,
                            preferred_term: sign.preferred_term,
                            assertion: AssertionStatus::Negated,
                            value: None,
                        },
                        matches,
                    );
                }
            }
        }
    }
}

fn add_anatomical_tenderness_matches(
    field: SoapField,
    field_text: &str,
    normalized: &NormalizedText,
    tokens: &[ExamToken],
    matches: &mut Vec<FindingMatch>,
) {
    for head_index in 0..tokens.len() {
        if !matches!(tokens[head_index].text.as_str(), "tenderness" | "tender") {
            continue;
        }
        if head_index > 0 && matches!(tokens[head_index - 1].text.as_str(), "non" | "not" | "no") {
            continue;
        }

        let mut start = head_index;
        let lower = head_index.saturating_sub(4);
        while start > lower
            && anatomical_exam_modifier_token(tokens[start - 1].text.as_str())
            && !exam_phrase_boundary_between(
                field_text,
                tokens[start - 1].orig_end,
                tokens[start].orig_start,
            )
        {
            start -= 1;
        }
        if start == head_index {
            continue;
        }
        if start > 0 && exam_negation_token(tokens[start - 1].text.as_str()) {
            continue;
        }

        add_structured_exam_match_from_tokens(
            field,
            field_text,
            normalized,
            tokens,
            StructuredExamMatchSpec {
                start_token: start,
                end_token: head_index + 1,
                concept_id: "301399007",
                preferred_term: "Musculoskeletal tenderness",
                assertion: AssertionStatus::Affirmed,
                value: None,
            },
            matches,
        );
    }
}

struct StructuredExamMatchSpec<'a> {
    start_token: usize,
    end_token: usize,
    concept_id: &'a str,
    preferred_term: &'a str,
    assertion: AssertionStatus,
    value: Option<MeasuredValue>,
}

fn add_structured_exam_match_from_tokens(
    field: SoapField,
    field_text: &str,
    normalized: &NormalizedText,
    tokens: &[ExamToken],
    spec: StructuredExamMatchSpec<'_>,
    matches: &mut Vec<FindingMatch>,
) {
    let StructuredExamMatchSpec {
        start_token,
        end_token,
        concept_id,
        preferred_term,
        assertion,
        value,
    } = spec;

    if start_token >= end_token || end_token > tokens.len() {
        return;
    }
    let span_start = tokens[start_token].orig_start;
    let span_end = tokens[end_token - 1].orig_end;
    if !should_add_structured_exam_match(matches, field, concept_id, span_start, span_end) {
        return;
    }

    let normalized_start = tokens[start_token].normalized_start;
    let normalized_end = tokens[end_token - 1].normalized_end;
    let normalized_match = normalized.text[normalized_start..normalized_end].to_string();
    let rule_id = structured_exam_rule_id(assertion).to_string();
    matches.push(FindingMatch {
        concept_id: concept_id.to_string(),
        preferred_term: preferred_term.to_string(),
        field,
        span_start,
        span_end,
        matched_text: field_text[span_start..span_end].to_string(),
        normalized_match,
        term_source: "built-in-structured-exam-feature".to_string(),
        value,
        body_site: None,
        assertion,
        rule_ids: vec![rule_id],
        explanation: structured_exam_explanation(field, assertion),
    });
}

fn should_add_structured_exam_match(
    matches: &[FindingMatch],
    field: SoapField,
    concept_id: &str,
    span_start: usize,
    span_end: usize,
) -> bool {
    !matches.iter().any(|item| {
        item.field == field
            && ((item.concept_id == concept_id
                && spans_overlap(span_start, span_end, item.span_start, item.span_end))
                || (!item
                    .term_source
                    .starts_with("built-in-structured-exam-feature")
                    && spans_overlap(span_start, span_end, item.span_start, item.span_end)))
    })
}

fn structured_exam_rule_id(assertion: AssertionStatus) -> &'static str {
    match assertion {
        AssertionStatus::Normal => "ASSERT_NORMAL_PATIENT_EXAMINATION_FINDING",
        AssertionStatus::Negated => "ASSERT_NEGATED_PATIENT_EXAMINATION_FINDING",
        _ => "ASSERT_AFFIRMED_PATIENT_EXAMINATION_FINDING",
    }
}

fn structured_exam_explanation(field: SoapField, assertion: AssertionStatus) -> String {
    match assertion {
        AssertionStatus::Normal => format!(
            "Reported as a normal patient examination finding in the {} field.",
            field.as_str()
        ),
        AssertionStatus::Negated => format!(
            "Reported as a negated patient examination finding in the {} field.",
            field.as_str()
        ),
        _ => format!(
            "Accepted as an affirmed patient examination finding in the {} field; no suppression rule fired.",
            field.as_str()
        ),
    }
}

fn match_any_phrase_at(tokens: &[ExamToken], start: usize, phrases: &[&str]) -> Option<usize> {
    phrases
        .iter()
        .find_map(|phrase| match_phrase_at(tokens, start, phrase))
}

fn match_phrase_at(tokens: &[ExamToken], start: usize, phrase: &str) -> Option<usize> {
    let phrase_tokens = phrase.split(' ').collect::<Vec<_>>();
    if start + phrase_tokens.len() > tokens.len() {
        return None;
    }
    phrase_tokens
        .iter()
        .enumerate()
        .all(|(offset, expected)| tokens[start + offset].text == *expected)
        .then_some(start + phrase_tokens.len())
}

fn find_status_after_subject(
    tokens: &[ExamToken],
    subject_end: usize,
    statuses: &[ExamResultStatus],
) -> Option<(usize, usize, ExamResultStatus)> {
    if statuses.is_empty() {
        return None;
    }

    let mut search_start = subject_end;
    while tokens
        .get(search_start)
        .map(|token| structured_exam_linking_token(token.text.as_str()))
        .unwrap_or(false)
    {
        search_start += 1;
    }

    let search_limit = (search_start + 4).min(tokens.len());
    for status_start in search_start..search_limit {
        if status_start > search_start
            && !tokens[search_start..status_start]
                .iter()
                .all(|token| structured_exam_status_bridge_token(token.text.as_str()))
        {
            break;
        }
        for status in statuses {
            if let Some(status_end) = match_phrase_at(tokens, status_start, status.phrase) {
                return Some((status_start, status_end, *status));
            }
        }
    }

    None
}

fn find_status_before_subject(
    tokens: &[ExamToken],
    subject_start: usize,
    statuses: &[ExamResultStatus],
) -> Option<(usize, usize, ExamResultStatus)> {
    if statuses.is_empty() {
        return None;
    }

    for status in statuses {
        let len = status.phrase.split(' ').count();
        if subject_start >= len {
            let status_start = subject_start - len;
            if let Some(status_end) = match_phrase_at(tokens, status_start, status.phrase) {
                return Some((status_start, status_end, *status));
            }
        }
    }

    None
}

fn cranial_nerve_subject_end(tokens: &[ExamToken], start: usize) -> Option<usize> {
    if tokens.get(start)?.text == "cn" {
        let mut index = start + 1;
        while index < tokens.len() && roman_or_arabic_numeral(tokens[index].text.as_str()) {
            index += 1;
        }
        return (index > start + 1).then_some(index);
    }

    if let Some(end) = match_any_phrase_at(tokens, start, &["cranial nerve", "cranial nerves"]) {
        return Some(end);
    }

    None
}

fn roman_or_arabic_numeral(token: &str) -> bool {
    token.chars().all(|ch| ch.is_ascii_digit())
        || matches!(
            token,
            "i" | "ii" | "iii" | "iv" | "v" | "vi" | "vii" | "viii" | "ix" | "x" | "xi" | "xii"
        )
}

fn looks_like_exam_score(token: &str) -> bool {
    token.contains('/')
        && token
            .split('/')
            .all(|part| part.chars().all(|ch| ch.is_ascii_digit()))
}

fn score_is_normal(token: &str) -> bool {
    let mut parts = token.split('/');
    let Some(left) = parts.next() else {
        return false;
    };
    let Some(right) = parts.next() else {
        return false;
    };
    parts.next().is_none() && left == right
}

fn exam_negation_token(token: &str) -> bool {
    matches!(token, "no" | "without" | "nil")
}

fn negated_exam_sign_gap_is_clear(
    tokens: &[ExamToken],
    start: usize,
    end: usize,
    allow_anatomical_modifiers: bool,
) -> bool {
    tokens[start..end].iter().all(|token| {
        matches!(
            token.text.as_str(),
            "any" | "evidence" | "of" | "sign" | "signs" | "and" | "or" | "nor"
        ) || known_exam_sign_head(token.text.as_str())
            || (allow_anatomical_modifiers && anatomical_exam_modifier_token(token.text.as_str()))
    })
}

fn known_exam_sign_head(token: &str) -> bool {
    NEGATED_EXAM_SIGNS
        .iter()
        .any(|sign| sign.heads.contains(&token))
}

fn structured_exam_linking_token(token: &str) -> bool {
    matches!(token, "is" | "are" | "was" | "were")
}

fn structured_exam_status_bridge_token(token: &str) -> bool {
    matches!(
        token,
        "and"
            | "or"
            | "plus"
            | "both"
            | "bilaterally"
            | "coordination"
            | "gait"
            | "power"
            | "reflex"
            | "reflexes"
            | "range"
            | "movement"
            | "tone"
            | "sensation"
            | "non"
            | "tender"
            | "nontender"
    )
}

fn anatomical_exam_modifier_token(token: &str) -> bool {
    matches!(
        token,
        "abdominal"
            | "ankle"
            | "arm"
            | "back"
            | "breast"
            | "calf"
            | "cervical"
            | "chest"
            | "elbow"
            | "epigastric"
            | "fossa"
            | "foot"
            | "genital"
            | "hand"
            | "hip"
            | "iliac"
            | "joint"
            | "knee"
            | "lumbar"
            | "mastoid"
            | "muscle"
            | "neck"
            | "paraspinal"
            | "renal"
            | "sacroiliac"
            | "scalp"
            | "skeletal"
            | "spinal"
            | "spine"
            | "sternum"
            | "temporal"
            | "testicle"
            | "testis"
            | "thoracic"
            | "wrist"
    )
}

fn exam_phrase_boundary_between(text: &str, start: usize, end: usize) -> bool {
    if start >= end || end > text.len() {
        return false;
    }
    text[start..end].chars().any(|ch| {
        matches!(
            ch,
            ':' | ';' | '.' | '\n' | '\r' | '-' | '\u{2013}' | '\u{2014}'
        )
    })
}

fn spans_overlap(left_start: usize, left_end: usize, right_start: usize, right_end: usize) -> bool {
    left_start < right_end && left_end > right_start
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
    extraction_kind: ExtractionKind,
) -> Option<crate::context::AssertionDecision> {
    if matches!(extraction_kind, ExtractionKind::Observable) {
        if let Some(decision) = observable_context_decision(raw, field_text) {
            return Some(decision);
        }
    }

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

fn observable_context_decision(
    raw: &RawMatch,
    field_text: &str,
) -> Option<crate::context::AssertionDecision> {
    if is_pulse_observable(raw) && raw.value.is_none() {
        return Some(observable_ambiguous_decision(
            "CTX_OBSERVABLE_PULSE_WITHOUT_NUMERIC_VALUE",
            "Suppressed: pulse observable mentions require a numeric value; qualitative peripheral pulse examination belongs in examination findings.",
        ));
    }

    if is_sensory_perception_observable(raw)
        && (raw.value.is_none() || has_qualitative_neurovascular_context(raw, field_text))
    {
        return Some(observable_ambiguous_decision(
            "CTX_OBSERVABLE_SENSATION_IN_EXAM_CONTEXT",
            "Suppressed: sensation observable mention appears in a qualitative neurovascular/peripheral examination context.",
        ));
    }

    if is_single_letter_respiratory_rate(raw) && looks_like_laterality_score_value(raw) {
        return Some(observable_ambiguous_decision(
            "CTX_OBSERVABLE_RESP_RATE_SIDE_LABEL",
            "Suppressed: single-letter R is acting as a right-side label for an examination score, not respiratory rate.",
        ));
    }

    if is_qualitative_exam_observable(raw)
        && raw.value.is_none()
        && has_qualitative_exam_context(raw, field_text)
    {
        return Some(observable_ambiguous_decision(
            "CTX_OBSERVABLE_QUALITATIVE_EXAM_CONTEXT",
            "Suppressed: observable mention appears in qualitative examination wording rather than a measured observation.",
        ));
    }

    None
}

fn observable_ambiguous_decision(
    rule_id: &str,
    explanation: &str,
) -> crate::context::AssertionDecision {
    crate::context::AssertionDecision {
        accepted: false,
        assertion: AssertionStatus::Ambiguous,
        rule_ids: vec![rule_id.to_string()],
        explanation: explanation.to_string(),
    }
}

fn is_pulse_observable(raw: &RawMatch) -> bool {
    let preferred = normalize_term(&raw.preferred_term);
    matches!(preferred.as_str(), "pulse" | "pulse rate")
        || matches!(raw.normalized_match.as_str(), "pulse" | "pulses")
}

fn is_sensory_perception_observable(raw: &RawMatch) -> bool {
    let preferred = normalize_term(&raw.preferred_term);
    matches!(
        preferred.as_str(),
        "sensory perception" | "sensation" | "sense"
    ) || matches!(raw.normalized_match.as_str(), "sensation" | "sensory")
}

fn is_single_letter_respiratory_rate(raw: &RawMatch) -> bool {
    normalize_term(&raw.preferred_term) == "respiratory rate"
        && raw.normalized_match == "r"
        && raw.matched_text.eq_ignore_ascii_case("r")
}

fn looks_like_laterality_score_value(raw: &RawMatch) -> bool {
    let Some(value) = raw.value.as_ref() else {
        return false;
    };

    value.text.contains('/')
        || value
            .unit
            .as_deref()
            .map(|unit| matches!(unit, "L" | "R" | "l" | "r"))
            .unwrap_or(false)
}

fn has_qualitative_neurovascular_context(raw: &RawMatch, field_text: &str) -> bool {
    let (window_start, window_end) = context_window(field_text, raw.span_start, raw.span_end, 80);
    let normalized = normalize_clinical_text(&field_text[window_start..window_end], raw.field).text;
    let tokens = normalized
        .split(' ')
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();

    tokens.iter().any(|token| {
        matches!(
            *token,
            "intact"
                | "reduced"
                | "absent"
                | "normal"
                | "palpable"
                | "present"
                | "monofilament"
                | "vibration"
                | "distal"
                | "dp"
                | "pt"
                | "pulses"
                | "pulse"
                | "neurovascular"
                | "peripheral"
        )
    })
}

fn is_qualitative_exam_observable(raw: &RawMatch) -> bool {
    let preferred = normalize_term(&raw.preferred_term);
    preferred.contains("range of movement")
        || preferred.starts_with("movement of ")
        || matches!(
            preferred.as_str(),
            "coordination" | "flexion" | "extension" | "gait" | "movement" | "reflex"
        )
        || matches!(
            raw.normalized_match.as_str(),
            "coordination"
                | "rom"
                | "range of movement"
                | "flexion"
                | "extension"
                | "gait"
                | "reflex"
                | "reflexes"
        )
}

fn has_qualitative_exam_context(raw: &RawMatch, field_text: &str) -> bool {
    let (window_start, window_end) = context_window(field_text, raw.span_start, raw.span_end, 80);
    let normalized = normalize_clinical_text(&field_text[window_start..window_end], raw.field).text;
    let tokens = normalized
        .split(' ')
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();

    tokens.iter().any(|token| {
        matches!(
            *token,
            "rom"
                | "range"
                | "movement"
                | "coordination"
                | "reflex"
                | "reflexes"
                | "limited"
                | "reduced"
                | "full"
                | "painful"
                | "normal"
                | "symmetrical"
                | "symmetric"
                | "intact"
                | "antalgic"
                | "uncomfortable"
                | "swelling"
                | "discomfort"
                | "tenderness"
        )
    })
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

fn non_affirmed_match_explanation(
    extraction_kind: ExtractionKind,
    field: SoapField,
    decision: &crate::context::AssertionDecision,
) -> String {
    let assertion = match decision.assertion {
        AssertionStatus::Negated => "negated",
        AssertionStatus::Uncertain => "uncertain",
        _ => "non-affirmed",
    };
    let kind = match extraction_kind {
        ExtractionKind::ExaminationFinding => "examination finding",
        ExtractionKind::Finding => "finding",
        ExtractionKind::Observable => "observable entity",
        ExtractionKind::Diagnosis => "diagnosis/disorder",
    };
    let reason = decision
        .explanation
        .strip_prefix("Suppressed: ")
        .unwrap_or(decision.explanation.as_str())
        .trim_end_matches('.');
    format!(
        "Reported as a {assertion} patient {kind} in the {} field: {reason}.",
        field.as_str()
    )
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

fn to_finding_match(
    raw: RawMatch,
    rule_ids: Vec<String>,
    explanation: String,
    body_site: Option<BodySiteMatch>,
    assertion: AssertionStatus,
) -> FindingMatch {
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
        body_site,
        assertion,
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

fn body_site_match_from_raw(raw: &RawMatch) -> BodySiteMatch {
    BodySiteMatch {
        concept_id: raw.concept_id.clone(),
        preferred_term: raw.preferred_term.clone(),
        span_start: raw.span_start,
        span_end: raw.span_end,
        matched_text: raw.matched_text.clone(),
        normalized_match: raw.normalized_match.clone(),
        term_source: raw.pattern_source.clone(),
    }
}

fn symptom_already_implies_body_site(
    raw: &RawMatch,
    body_site_matcher: &TerminologyMatcher,
) -> bool {
    let normalized_preferred = normalize_term(&raw.preferred_term);
    if contains_compound_site_ache(&normalized_preferred)
        || contains_compound_site_ache(&raw.normalized_match)
    {
        return true;
    }

    [raw.preferred_term.as_str(), raw.normalized_match.as_str()]
        .iter()
        .any(|text| {
            body_site_matcher
                .find_in_field(SoapField::History, text, false)
                .into_iter()
                .any(|site| !generic_body_site(&site))
        })
}

fn body_site_association_score(
    symptom: &RawMatch,
    site: &RawMatch,
    field_text: &str,
) -> Option<usize> {
    if ranges_overlap(
        symptom.span_start,
        symptom.span_end,
        site.span_start,
        site.span_end,
    ) {
        return None;
    }

    let (gap_start, gap_end, direction_penalty) = if site.span_end <= symptom.span_start {
        (site.span_end, symptom.span_start, 1)
    } else {
        (symptom.span_end, site.span_start, 0)
    };
    let gap = field_text.get(gap_start..gap_end)?;
    if contains_body_site_boundary(gap) {
        return None;
    }

    let gap_tokens = normalize_term(gap)
        .split(' ')
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if gap_tokens.len() > 4
        || gap_tokens
            .iter()
            .any(|token| !allowed_body_site_gap_token(token))
    {
        return None;
    }

    let char_gap = gap_end.saturating_sub(gap_start);
    if char_gap > 48 {
        return None;
    }

    Some(char_gap + (gap_tokens.len() * 8) + direction_penalty)
}

fn heading_body_site_for_match(
    symptom: &RawMatch,
    field_text: &str,
    body_site_matches: &[RawMatch],
) -> Option<BodySiteMatch> {
    body_site_matches
        .iter()
        .filter(|site| {
            site.field == symptom.field
                && !generic_body_site(site)
                && site.span_end <= symptom.span_start
                && symptom.span_start.saturating_sub(site.span_end) <= 300
                && body_site_heading_starts_scope(field_text, site.span_start, site.span_end)
                && body_site_heading_scope_reaches(field_text, site.span_end, symptom.span_start)
        })
        .max_by_key(|site| site.span_start)
        .map(body_site_match_from_raw)
}

fn body_site_heading_starts_scope(field_text: &str, site_start: usize, site_end: usize) -> bool {
    let prefix_start = previous_hard_boundary(field_text, site_start);
    let Some(prefix) = field_text.get(prefix_start..site_start) else {
        return false;
    };
    let normalized_prefix = normalize_term(prefix);
    if !normalized_prefix
        .split(' ')
        .filter(|token| !token.is_empty())
        .all(heading_prefix_token)
    {
        return false;
    }

    let Some(after) = field_text.get(site_end..) else {
        return false;
    };
    heading_separator(after.trim_start())
}

fn heading_separator(value: &str) -> bool {
    value
        .chars()
        .next()
        .map(|ch| matches!(ch, ':' | '-' | '\u{2013}' | '\u{2014}'))
        .unwrap_or(false)
        || value
            .chars()
            .next()
            .map(|ch| !ch.is_alphanumeric())
            .unwrap_or(false)
        || value.starts_with('\u{00e2}')
        || value.starts_with("\u{00e2}\u{20ac}\u{201c}")
        || value.starts_with("\u{00e2}\u{20ac}\u{201d}")
}

fn body_site_heading_scope_reaches(
    field_text: &str,
    site_end: usize,
    symptom_start: usize,
) -> bool {
    let Some(gap) = field_text.get(site_end..symptom_start) else {
        return false;
    };
    !gap.contains("\n\n") && !gap.contains("\r\n\r\n")
}

fn previous_hard_boundary(field_text: &str, before: usize) -> usize {
    field_text
        .get(..before)
        .and_then(|prefix| {
            prefix
                .char_indices()
                .rev()
                .find(|(_, ch)| matches!(ch, '.' | ';' | '\n' | '\r'))
                .map(|(idx, ch)| idx + ch.len_utf8())
        })
        .unwrap_or(0)
}

fn heading_prefix_token(token: &str) -> bool {
    matches!(
        token,
        "r" | "l" | "right" | "left" | "bilateral" | "bilaterally" | "both" | "the" | "a" | "an"
    )
}

fn ranges_overlap(
    left_start: usize,
    left_end: usize,
    right_start: usize,
    right_end: usize,
) -> bool {
    left_start < right_end && right_start < left_end
}

fn contains_body_site_boundary(value: &str) -> bool {
    value
        .chars()
        .any(|ch| matches!(ch, '.' | ';' | '\n' | '\r'))
}

fn allowed_body_site_gap_token(token: &str) -> bool {
    matches!(
        token,
        "a" | "an"
            | "the"
            | "in"
            | "on"
            | "of"
            | "at"
            | "to"
            | "over"
            | "around"
            | "under"
            | "from"
            | "into"
            | "left"
            | "right"
            | "r"
            | "l"
            | "bilateral"
            | "bilaterally"
            | "both"
            | "upper"
            | "lower"
            | "inner"
            | "outer"
            | "anterior"
            | "posterior"
            | "medial"
            | "lateral"
            | "proximal"
            | "distal"
            | "front"
            | "back"
            | "and"
    )
}

fn generic_body_site(site: &RawMatch) -> bool {
    matches!(
        normalize_term(&site.preferred_term).as_str(),
        "body"
            | "body structure"
            | "entire body"
            | "body part"
            | "anatomical structure"
            | "organ"
            | "joint"
            | "structure"
    ) || matches!(
        site.normalized_match.as_str(),
        "body"
            | "body structure"
            | "entire body"
            | "body part"
            | "anatomical structure"
            | "organ"
            | "joint"
            | "structure"
    )
}

fn contains_compound_site_ache(normalized: &str) -> bool {
    normalized.split(' ').any(|token| {
        matches!(
            token,
            "earache"
                | "otalgia"
                | "headache"
                | "cephalalgia"
                | "backache"
                | "toothache"
                | "odontalgia"
                | "stomachache"
                | "bellyache"
        )
    })
}
