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
use std::collections::HashSet;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct Extractor {
    artefact: TerminologyArtefact,
    matcher: TerminologyMatcher,
    body_site_matcher: Option<TerminologyMatcher>,
    body_site_artefact: Option<TerminologyArtefact>,
}

impl Extractor {
    pub fn new(artefact: TerminologyArtefact) -> Result<Self> {
        let matcher = TerminologyMatcher::new(&artefact)?;
        Ok(Self {
            artefact,
            matcher,
            body_site_matcher: None,
            body_site_artefact: None,
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
            body_site_artefact: Some(body_site_artefact),
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
            let mut body_site_matches = if matches!(extraction_kind, ExtractionKind::Finding) {
                self.body_site_matcher
                    .as_ref()
                    .map(|matcher| matcher.find_in_field(field, text, false))
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            if matches!(extraction_kind, ExtractionKind::Finding) {
                if let Some(body_site_artefact) = self.body_site_artefact.as_ref() {
                    add_derived_body_site_matches(
                        field,
                        text,
                        body_site_artefact,
                        &mut body_site_matches,
                    );
                }
            }
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
                    if matches!(extraction_kind, ExtractionKind::Finding)
                        && self.body_site_matcher.is_some()
                        && body_site.is_none()
                        && site_dependent_broad_finding(&raw)
                    {
                        if request.include_suppressed {
                            suppressed.push(to_suppressed_match(
                                raw,
                                AssertionStatus::Ambiguous,
                                vec!["CTX_BROAD_FINDING_WITHOUT_BODY_SITE".to_string()],
                                "Suppressed: broad site-dependent findings require a linked body site."
                                    .to_string(),
                            ));
                        }
                        continue;
                    }
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

        dedupe_finding_matches(&mut matches);
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
            .or_else(|| topic_body_site_for_match(raw, field_text, body_site_matches))
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

fn dedupe_finding_matches(matches: &mut Vec<FindingMatch>) {
    let mut seen = HashSet::new();
    matches.retain(|item| {
        seen.insert((
            item.field,
            item.span_start,
            item.span_end,
            item.concept_id.clone(),
            item.assertion,
        ))
    });
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

struct NamedExamTestFeature {
    concept_id: &'static str,
    preferred_term: &'static str,
    subjects: &'static [&'static str],
    statuses_after: &'static [ExamResultStatus],
    statuses_before: &'static [ExamResultStatus],
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

const STATUS_REDUCED: &[ExamResultStatus] = &[
    ExamResultStatus {
        phrase: "reduced",
        assertion: AssertionStatus::Affirmed,
    },
    ExamResultStatus {
        phrase: "diminished",
        assertion: AssertionStatus::Affirmed,
    },
];

const STATUS_REDUCED_LIMITED: &[ExamResultStatus] = &[
    ExamResultStatus {
        phrase: "reduced",
        assertion: AssertionStatus::Affirmed,
    },
    ExamResultStatus {
        phrase: "diminished",
        assertion: AssertionStatus::Affirmed,
    },
    ExamResultStatus {
        phrase: "limited",
        assertion: AssertionStatus::Affirmed,
    },
    ExamResultStatus {
        phrase: "restricted",
        assertion: AssertionStatus::Affirmed,
    },
];

const STATUS_ABSENT: &[ExamResultStatus] = &[ExamResultStatus {
    phrase: "absent",
    assertion: AssertionStatus::Affirmed,
}];

const STATUS_BRISK: &[ExamResultStatus] = &[ExamResultStatus {
    phrase: "brisk",
    assertion: AssertionStatus::Affirmed,
}];

const STATUS_PROLONGED: &[ExamResultStatus] = &[ExamResultStatus {
    phrase: "prolonged",
    assertion: AssertionStatus::Affirmed,
}];

const STATUS_BRISK_OR_NORMAL: &[ExamResultStatus] = &[
    ExamResultStatus {
        phrase: "brisk",
        assertion: AssertionStatus::Normal,
    },
    ExamResultStatus {
        phrase: "normal",
        assertion: AssertionStatus::Normal,
    },
];

const STATUS_NEGATIVE: &[ExamResultStatus] = &[
    ExamResultStatus {
        phrase: "negative",
        assertion: AssertionStatus::Negated,
    },
    ExamResultStatus {
        phrase: "neg",
        assertion: AssertionStatus::Negated,
    },
];

const STATUS_POSITIVE_OR_NEGATIVE: &[ExamResultStatus] = &[
    ExamResultStatus {
        phrase: "positive",
        assertion: AssertionStatus::Affirmed,
    },
    ExamResultStatus {
        phrase: "pos",
        assertion: AssertionStatus::Affirmed,
    },
    ExamResultStatus {
        phrase: "negative",
        assertion: AssertionStatus::Negated,
    },
    ExamResultStatus {
        phrase: "neg",
        assertion: AssertionStatus::Negated,
    },
];

const STATUS_POSITIVE_PAIN_CLICK_OR_NEGATIVE: &[ExamResultStatus] = &[
    ExamResultStatus {
        phrase: "positive",
        assertion: AssertionStatus::Affirmed,
    },
    ExamResultStatus {
        phrase: "pos",
        assertion: AssertionStatus::Affirmed,
    },
    ExamResultStatus {
        phrase: "pain click",
        assertion: AssertionStatus::Affirmed,
    },
    ExamResultStatus {
        phrase: "pain and click",
        assertion: AssertionStatus::Affirmed,
    },
    ExamResultStatus {
        phrase: "negative",
        assertion: AssertionStatus::Negated,
    },
    ExamResultStatus {
        phrase: "neg",
        assertion: AssertionStatus::Negated,
    },
];

const STATUS_SNT_OR_SOFT_NONTENDER: &[ExamResultStatus] = &[
    ExamResultStatus {
        phrase: "soft",
        assertion: AssertionStatus::Normal,
    },
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

const REFLEX_SUBJECTS: &[&str] = &[
    "reflex",
    "reflexes",
    "aj",
    "kj",
    "ankle jerk",
    "knee jerk",
    "biceps",
    "brachioradialis",
    "supinator",
    "triceps",
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
        concept_id: "300280008",
        preferred_term: "Pharynx normal",
        subjects: &["throat", "pharynx"],
        statuses_after: STATUS_NORMAL,
        statuses_before: STATUS_NORMAL,
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
        concept_id: "835279003",
        preferred_term: "Decreased reflex",
        subjects: REFLEX_SUBJECTS,
        statuses_after: STATUS_REDUCED,
        statuses_before: STATUS_REDUCED,
    },
    StructuredExamFeature {
        concept_id: "37280007",
        preferred_term: "Absent reflex",
        subjects: REFLEX_SUBJECTS,
        statuses_after: STATUS_ABSENT,
        statuses_before: STATUS_ABSENT,
    },
    StructuredExamFeature {
        concept_id: "86854008",
        preferred_term: "Hyperreflexia",
        subjects: REFLEX_SUBJECTS,
        statuses_after: STATUS_BRISK,
        statuses_before: STATUS_BRISK,
    },
    StructuredExamFeature {
        concept_id: "397974008",
        preferred_term: "Hypesthesia",
        subjects: &["sensation", "sensory"],
        statuses_after: STATUS_REDUCED,
        statuses_before: STATUS_REDUCED,
    },
    StructuredExamFeature {
        concept_id: "397974008",
        preferred_term: "Hypesthesia",
        subjects: &["sensation", "sensory"],
        statuses_after: STATUS_ABSENT,
        statuses_before: STATUS_ABSENT,
    },
    StructuredExamFeature {
        concept_id: "299934008",
        preferred_term: "Impaired vibration sensation",
        subjects: &["vibration", "vibration sense"],
        statuses_after: STATUS_REDUCED,
        statuses_before: STATUS_REDUCED,
    },
    StructuredExamFeature {
        concept_id: "103003004",
        preferred_term: "Impaired body position sense",
        subjects: &["proprioception", "position sense"],
        statuses_after: STATUS_REDUCED,
        statuses_before: STATUS_REDUCED,
    },
    StructuredExamFeature {
        concept_id: "390932001",
        preferred_term: "10g monofilament sensation absent",
        subjects: &["monofilament", "10g monofilament", "monofilament sensation"],
        statuses_after: STATUS_ABSENT,
        statuses_before: STATUS_ABSENT,
    },
    StructuredExamFeature {
        concept_id: "301161008",
        preferred_term: "Peripheral pulse absent",
        subjects: &["pulse", "pulses", "pedal pulses"],
        statuses_after: STATUS_ABSENT,
        statuses_before: STATUS_ABSENT,
    },
    StructuredExamFeature {
        concept_id: "301170006",
        preferred_term: "Dorsalis pulse absent",
        subjects: &["dp", "dorsalis pedis"],
        statuses_after: STATUS_ABSENT,
        statuses_before: STATUS_ABSENT,
    },
    StructuredExamFeature {
        concept_id: "301169005",
        preferred_term: "Posterior tibial pulse absent",
        subjects: &["pt", "posterior tibial"],
        statuses_after: STATUS_ABSENT,
        statuses_before: STATUS_ABSENT,
    },
    StructuredExamFeature {
        concept_id: "45332005",
        preferred_term: "Normal capillary filling",
        subjects: &["crt", "capillary refill", "capillary refill time"],
        statuses_after: STATUS_BRISK_OR_NORMAL,
        statuses_before: &[],
    },
    StructuredExamFeature {
        concept_id: "50427001",
        preferred_term: "Increased capillary filling time",
        subjects: &["crt", "capillary refill", "capillary refill time"],
        statuses_after: STATUS_PROLONGED,
        statuses_before: STATUS_PROLONGED,
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
        concept_id: "70733008",
        preferred_term: "Limitation of joint movement",
        subjects: &["flexion", "extension", "joint movement"],
        statuses_after: STATUS_REDUCED_LIMITED,
        statuses_before: STATUS_REDUCED_LIMITED,
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

const NAMED_EXAM_TEST_FEATURES: &[NamedExamTestFeature] = &[
    NamedExamTestFeature {
        concept_id: "366448008",
        preferred_term: "Finding of straight leg raise",
        subjects: &["slr", "straight leg raise", "straight leg raising"],
        statuses_after: STATUS_POSITIVE_OR_NEGATIVE,
        statuses_before: STATUS_POSITIVE_OR_NEGATIVE,
    },
    NamedExamTestFeature {
        concept_id: "82668000",
        preferred_term: "Crossed leg raising sign",
        subjects: &[
            "crossed slr",
            "crossed straight leg raise",
            "crossed straight leg raising",
        ],
        statuses_after: STATUS_POSITIVE_OR_NEGATIVE,
        statuses_before: STATUS_POSITIVE_OR_NEGATIVE,
    },
    NamedExamTestFeature {
        concept_id: "19411004",
        preferred_term: "Spurling sign",
        subjects: &["spurling", "spurling s", "spurling test", "spurling s test"],
        statuses_after: STATUS_POSITIVE_OR_NEGATIVE,
        statuses_before: STATUS_POSITIVE_OR_NEGATIVE,
    },
    NamedExamTestFeature {
        concept_id: "299391009",
        preferred_term: "McMurray test positive",
        subjects: &["mcmurray", "mcmurray s", "mcmurray test", "mcmurray s test"],
        statuses_after: STATUS_POSITIVE_PAIN_CLICK_OR_NEGATIVE,
        statuses_before: STATUS_POSITIVE_OR_NEGATIVE,
    },
    NamedExamTestFeature {
        concept_id: "299393007",
        preferred_term: "Lachman test positive",
        subjects: &["lachman", "lachman s", "lachman test", "lachman s test"],
        statuses_after: STATUS_POSITIVE_OR_NEGATIVE,
        statuses_before: STATUS_POSITIVE_OR_NEGATIVE,
    },
    NamedExamTestFeature {
        concept_id: "366605006",
        preferred_term: "Anterior drawer test - finding",
        subjects: &["anterior drawer", "anterior drawer test"],
        statuses_after: STATUS_POSITIVE_OR_NEGATIVE,
        statuses_before: STATUS_POSITIVE_OR_NEGATIVE,
    },
    NamedExamTestFeature {
        concept_id: "299849001",
        preferred_term: "Hoffman's reflex positive",
        subjects: &[
            "hoffman",
            "hoffman s",
            "hoffmann",
            "hoffmann s",
            "hoffman reflex",
            "hoffmann reflex",
        ],
        statuses_after: STATUS_POSITIVE_OR_NEGATIVE,
        statuses_before: STATUS_POSITIVE_OR_NEGATIVE,
    },
    NamedExamTestFeature {
        concept_id: "39051003",
        preferred_term: "Kernig's sign",
        subjects: &["kernig", "kernig s", "kernig sign", "kernig s sign"],
        statuses_after: STATUS_POSITIVE_OR_NEGATIVE,
        statuses_before: STATUS_POSITIVE_OR_NEGATIVE,
    },
    NamedExamTestFeature {
        concept_id: "82345001",
        preferred_term: "Brudzinski's sign",
        subjects: &[
            "brudzinski",
            "brudzinski s",
            "brudzinski sign",
            "brudzinski s sign",
        ],
        statuses_after: STATUS_POSITIVE_OR_NEGATIVE,
        statuses_before: STATUS_POSITIVE_OR_NEGATIVE,
    },
    NamedExamTestFeature {
        concept_id: "716521001",
        preferred_term: "Ottawa ankle rules test negative",
        subjects: &[
            "ottawa ankle",
            "ottawa ankle rules",
            "ottawa ankle foot rules",
            "ottawa ankle and foot rules",
        ],
        statuses_after: STATUS_NEGATIVE,
        statuses_before: STATUS_NEGATIVE,
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
    add_named_exam_test_matches(field, field_text, &normalized, &tokens, matches);
    add_negated_exam_sign_matches(field, field_text, &normalized, &tokens, matches);
    add_negated_swelling_matches(field, field_text, &normalized, &tokens, matches);
    add_anatomical_tenderness_matches(field, field_text, &normalized, &tokens, matches);
    add_anatomical_surface_sign_matches(field, field_text, &normalized, &tokens, matches);
    add_contextual_discharge_matches(field, field_text, &normalized, &tokens, matches);
    add_joint_effusion_matches(field, field_text, &normalized, &tokens, matches);
    add_muscle_weakness_matches(field, field_text, &normalized, &tokens, matches);
    add_grip_limited_by_pain_matches(field, field_text, &normalized, &tokens, matches);
    add_capillary_refill_threshold_matches(field, field_text, &normalized, &tokens, matches);
    add_antalgic_gait_matches(field, field_text, &normalized, &tokens, matches);
    add_breast_lump_matches(field, field_text, &normalized, &tokens, matches);
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
                if feature.concept_id == "246581004"
                    && subject_start > 0
                    && tokens[subject_start - 1].text == "red"
                {
                    continue;
                }

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
        if let Some(value_token) = tokens.get(subject_end) {
            if looks_like_exam_score(value_token.text.as_str()) {
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

        let scan_limit = power_score_clause_limit(field_text, tokens, subject_end);
        for score_index in subject_end..scan_limit {
            if !looks_like_exam_score(tokens[score_index].text.as_str()) {
                continue;
            }
            let assertion = if score_is_normal(tokens[score_index].text.as_str()) {
                AssertionStatus::Normal
            } else {
                AssertionStatus::Affirmed
            };
            let value = MeasuredValue {
                text: field_text[tokens[score_index].orig_start..tokens[score_index].orig_end]
                    .to_string(),
                unit: None,
                span_start: tokens[score_index].orig_start,
                span_end: tokens[score_index].orig_end,
            };
            let context_start = power_score_context_start(tokens, subject_end, score_index);

            add_structured_exam_match_from_tokens(
                field,
                field_text,
                normalized,
                tokens,
                StructuredExamMatchSpec {
                    start_token: context_start,
                    end_token: score_index + 1,
                    concept_id: "249948009",
                    preferred_term: "Grade of muscle power",
                    assertion,
                    value: Some(value),
                },
                matches,
            );
        }
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

fn add_named_exam_test_matches(
    field: SoapField,
    field_text: &str,
    normalized: &NormalizedText,
    tokens: &[ExamToken],
    matches: &mut Vec<FindingMatch>,
) {
    for feature in NAMED_EXAM_TEST_FEATURES {
        for subject_start in 0..tokens.len() {
            for subject in feature.subjects {
                let Some(subject_end) = match_phrase_at(tokens, subject_start, subject) else {
                    continue;
                };

                if let Some((_, status_end, status)) =
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
                }

                if let Some((status_start, _, status)) =
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

fn add_negated_swelling_matches(
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
        for head_index in negation_index + 1..search_end {
            if !matches!(
                tokens[head_index].text.as_str(),
                "swelling" | "swollen" | "oedema" | "edema"
            ) || !negated_exam_sign_gap_is_clear(tokens, negation_index + 1, head_index, true)
            {
                continue;
            }

            let Some((concept_id, preferred_term)) =
                negated_swelling_concept(tokens, negation_index, head_index)
            else {
                continue;
            };

            add_structured_exam_match_from_tokens(
                field,
                field_text,
                normalized,
                tokens,
                StructuredExamMatchSpec {
                    start_token: negation_index,
                    end_token: head_index + 1,
                    concept_id,
                    preferred_term,
                    assertion: AssertionStatus::Negated,
                    value: None,
                },
                matches,
            );
        }
    }
}

fn negated_swelling_concept(
    tokens: &[ExamToken],
    negation_index: usize,
    head_index: usize,
) -> Option<(&'static str, &'static str)> {
    let context_start = negation_index.saturating_sub(8);
    if tokens[context_start..head_index]
        .iter()
        .any(|token| musculoskeletal_exam_topic_token(token.text.as_str()))
    {
        return Some(("271771009", "Joint swelling"));
    }

    if tokens[negation_index + 1..head_index]
        .iter()
        .any(|token| matches!(token.text.as_str(), "periorbital" | "eyelid" | "eye"))
    {
        return None;
    }

    Some(("298349001", "Soft tissue swelling"))
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
        if start < head_index {
            if start > 0 && exam_negation_token(tokens[start - 1].text.as_str()) {
                continue;
            }

            let (concept_id, preferred_term) =
                tenderness_concept_for_modifier_tokens(&tokens[start..head_index]);
            add_structured_exam_match_from_tokens(
                field,
                field_text,
                normalized,
                tokens,
                StructuredExamMatchSpec {
                    start_token: start,
                    end_token: head_index + 1,
                    concept_id,
                    preferred_term,
                    assertion: AssertionStatus::Affirmed,
                    value: None,
                },
                matches,
            );
        }

        if let Some((location_start, location_end)) =
            tenderness_location_span(tokens, head_index, field_text)
        {
            let start = preceding_tenderness_context_start(tokens, location_start, head_index);
            let (concept_id, preferred_term) =
                tenderness_concept_for_modifier_tokens(&tokens[start..location_end]);
            add_structured_exam_match_from_tokens(
                field,
                field_text,
                normalized,
                tokens,
                StructuredExamMatchSpec {
                    start_token: start,
                    end_token: location_end,
                    concept_id,
                    preferred_term,
                    assertion: AssertionStatus::Affirmed,
                    value: None,
                },
                matches,
            );
        }

        if start == head_index {
            if let Some(heading_start) =
                heading_tenderness_modifier_start(tokens, head_index, field_text)
            {
                let (concept_id, preferred_term) =
                    tenderness_concept_for_modifier_tokens(&tokens[heading_start..head_index]);
                add_structured_exam_match_from_tokens(
                    field,
                    field_text,
                    normalized,
                    tokens,
                    StructuredExamMatchSpec {
                        start_token: heading_start,
                        end_token: head_index + 1,
                        concept_id,
                        preferred_term,
                        assertion: AssertionStatus::Affirmed,
                        value: None,
                    },
                    matches,
                );
            }
        }

        if tokens[head_index].text != "tender" {
            continue;
        }

        let mut end = head_index + 1;
        let upper = (head_index + 6).min(tokens.len());
        while end < upper
            && post_tender_modifier_token(tokens[end].text.as_str())
            && !exam_phrase_boundary_between(
                field_text,
                tokens[end - 1].orig_end,
                tokens[end].orig_start,
            )
        {
            end += 1;
        }
        if end == head_index + 1 {
            continue;
        }

        let (concept_id, preferred_term) =
            tenderness_concept_for_modifier_tokens(&tokens[head_index + 1..end]);
        add_structured_exam_match_from_tokens(
            field,
            field_text,
            normalized,
            tokens,
            StructuredExamMatchSpec {
                start_token: head_index,
                end_token: end,
                concept_id,
                preferred_term,
                assertion: AssertionStatus::Affirmed,
                value: None,
            },
            matches,
        );
    }
}

fn tenderness_location_span(
    tokens: &[ExamToken],
    head_index: usize,
    field_text: &str,
) -> Option<(usize, usize)> {
    let mut index = head_index + 1;
    if index >= tokens.len() {
        return None;
    }

    let scan_limit = (head_index + 12).min(tokens.len());
    while index < scan_limit {
        if exam_phrase_boundary_between(
            field_text,
            tokens[index - 1].orig_end,
            tokens[index].orig_start,
        ) {
            return None;
        }
        if tenderness_location_connector(tokens, index) {
            let mut end = index + 1;
            let mut saw_location = false;
            while end < scan_limit
                && !exam_phrase_boundary_between(
                    field_text,
                    tokens[end - 1].orig_end,
                    tokens[end].orig_start,
                )
                && tenderness_location_token(tokens[end].text.as_str())
            {
                if anatomical_exam_modifier_token(tokens[end].text.as_str()) {
                    saw_location = true;
                }
                end += 1;
            }
            if saw_location {
                return Some((index, end));
            }
            return None;
        }
        if !tenderness_bridge_token(tokens[index].text.as_str()) {
            return None;
        }
        index += 1;
    }

    None
}

fn preceding_tenderness_context_start(
    tokens: &[ExamToken],
    location_start: usize,
    head_index: usize,
) -> usize {
    let mut start = head_index;
    let lower = head_index.saturating_sub(4);
    while start > lower && tenderness_prefix_token(tokens[start - 1].text.as_str()) {
        start -= 1;
    }
    start.min(location_start)
}

fn tenderness_bridge_token(token: &str) -> bool {
    matches!(
        token,
        "and" | "plus" | "palpation" | "percussion" | "on" | "to" | "at" | "of"
    )
}

fn tenderness_location_connector(tokens: &[ExamToken], index: usize) -> bool {
    matches!(tokens[index].text.as_str(), "over" | "at" | "of" | "to")
        || (tokens[index].text == "on"
            && tokens
                .get(index + 1)
                .map(|token| !matches!(token.text.as_str(), "palpation" | "percussion"))
                .unwrap_or(true))
}

fn tenderness_location_token(token: &str) -> bool {
    anatomical_exam_modifier_token(token)
        || matches!(
            token,
            "and" | "plus" | "or" | "the" | "a" | "an" | "r" | "l" | "right" | "left" | "common"
        )
}

fn tenderness_prefix_token(token: &str) -> bool {
    degree_modifier_token(token)
        || anatomical_exam_modifier_token(token)
        || matches!(token, "point" | "focal" | "localized" | "localised")
}

fn heading_tenderness_modifier_start(
    tokens: &[ExamToken],
    head_index: usize,
    field_text: &str,
) -> Option<usize> {
    let mut status_start = head_index;
    while status_start > 0
        && degree_modifier_token(tokens[status_start - 1].text.as_str())
        && !exam_phrase_boundary_between(
            field_text,
            tokens[status_start - 1].orig_end,
            tokens[status_start].orig_start,
        )
    {
        status_start -= 1;
    }

    let mut start = status_start.checked_sub(1)?;
    if !anatomical_exam_modifier_token(tokens[start].text.as_str()) {
        return None;
    }

    let separator = field_text.get(tokens[start].orig_end..tokens[status_start].orig_start)?;
    if !heading_separator(separator.trim_start()) {
        return None;
    }

    let lower = start.saturating_sub(4);
    while start > lower
        && anatomical_exam_modifier_token(tokens[start - 1].text.as_str())
        && !exam_phrase_boundary_except_hyphen_between(
            field_text,
            tokens[start - 1].orig_end,
            tokens[start].orig_start,
        )
    {
        start -= 1;
    }

    if start > 0 && exam_negation_token(tokens[start - 1].text.as_str()) {
        return None;
    }

    Some(start)
}

fn tenderness_concept_for_modifier_tokens(tokens: &[ExamToken]) -> (&'static str, &'static str) {
    if tokens.iter().any(|token| token.text == "epigastric") {
        return ("301403003", "Tenderness of epigastric region");
    }
    if tokens
        .iter()
        .any(|token| abdominal_tenderness_modifier_token(token.text.as_str()))
    {
        return ("43478001", "Abdominal tenderness");
    }
    if tokens.iter().any(|token| token.text == "breast") {
        return ("55222007", "Tenderness of breast");
    }
    if tokens.iter().any(|token| {
        matches!(
            token.text.as_str(),
            "adnexal"
                | "adnexa"
                | "genital"
                | "suprapubic"
                | "testicle"
                | "testis"
                | "uterine"
                | "uterus"
        )
    }) {
        return ("301394002", "Genitourinary tenderness");
    }
    if tokens.iter().any(|token| {
        matches!(
            token.text.as_str(),
            "frontal" | "maxillary" | "sinus" | "sinuses"
        )
    }) {
        return ("278997003", "Tenderness of bone");
    }
    if tokens
        .iter()
        .any(|token| matches!(token.text.as_str(), "joint" | "joints"))
    {
        return ("110288007", "Tenderness of joint");
    }
    if tokens.iter().any(|token| token.text == "renal") {
        return ("102830001", "Renal angle tenderness");
    }

    ("301399007", "Musculoskeletal tenderness")
}

fn abdominal_tenderness_modifier_token(token: &str) -> bool {
    matches!(
        token,
        "abdomen" | "abdominal" | "fossa" | "iliac" | "quadrant"
    )
}

fn add_anatomical_surface_sign_matches(
    field: SoapField,
    field_text: &str,
    normalized: &NormalizedText,
    tokens: &[ExamToken],
    matches: &mut Vec<FindingMatch>,
) {
    for head_index in 0..tokens.len() {
        let concept = match tokens[head_index].text.as_str() {
            "erythema" | "erythematous" | "red" | "redness" => Some(("247441003", "Erythema")),
            "hot" | "warm" | "warmth" => Some(("707793005", "Hot skin")),
            _ => None,
        };
        let Some((concept_id, preferred_term)) = concept else {
            continue;
        };
        if preceding_negation_token(tokens, head_index, 5) {
            continue;
        }

        let Some(start) = surface_sign_context_start(tokens, head_index, field_text) else {
            continue;
        };
        add_structured_exam_match_from_tokens(
            field,
            field_text,
            normalized,
            tokens,
            StructuredExamMatchSpec {
                start_token: start,
                end_token: head_index + 1,
                concept_id,
                preferred_term,
                assertion: AssertionStatus::Affirmed,
                value: None,
            },
            matches,
        );
    }
}

fn surface_sign_context_start(
    tokens: &[ExamToken],
    head_index: usize,
    field_text: &str,
) -> Option<usize> {
    let lower = head_index.saturating_sub(6);
    let mut start = head_index;
    let mut saw_site = false;
    while start > lower
        && surface_sign_context_token(tokens[start - 1].text.as_str())
        && !exam_phrase_boundary_between(
            field_text,
            tokens[start - 1].orig_end,
            tokens[start].orig_start,
        )
    {
        start -= 1;
        if anatomical_exam_modifier_token(tokens[start].text.as_str()) {
            saw_site = true;
        }
    }

    if saw_site && start < head_index {
        Some(start)
    } else {
        None
    }
}

fn surface_sign_context_token(token: &str) -> bool {
    anatomical_exam_modifier_token(token)
        || degree_modifier_token(token)
        || matches!(
            token,
            "and" | "plus" | "congested" | "mucosa" | "mucosal" | "skin"
        )
}

fn add_contextual_discharge_matches(
    field: SoapField,
    field_text: &str,
    normalized: &NormalizedText,
    tokens: &[ExamToken],
    matches: &mut Vec<FindingMatch>,
) {
    for head_index in 0..tokens.len() {
        if !matches!(tokens[head_index].text.as_str(), "discharge" | "discharges") {
            continue;
        }

        if preceding_negation_token(tokens, head_index, 4) {
            continue;
        }

        let Some((start, end, concept_id, preferred_term)) =
            contextual_discharge_span(tokens, head_index, field_text)
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
                end_token: end,
                concept_id,
                preferred_term,
                assertion: AssertionStatus::Affirmed,
                value: None,
            },
            matches,
        );
    }
}

fn contextual_discharge_span(
    tokens: &[ExamToken],
    head_index: usize,
    field_text: &str,
) -> Option<(usize, usize, &'static str, &'static str)> {
    let mut start = head_index;
    let lower = head_index.saturating_sub(5);
    while start > lower
        && discharge_prefix_token(tokens[start - 1].text.as_str())
        && !exam_phrase_boundary_between(
            field_text,
            tokens[start - 1].orig_end,
            tokens[start].orig_start,
        )
    {
        start -= 1;
    }

    let scan_limit = (head_index + 8).min(tokens.len());
    let mut end = head_index + 1;
    let mut saw_vaginal = tokens[start..end]
        .iter()
        .any(|token| vaginal_context_token(token.text.as_str()));
    let mut saw_nasal = tokens[start..end]
        .iter()
        .any(|token| nasal_context_token(token.text.as_str()));
    while end < scan_limit
        && !exam_phrase_boundary_except_hyphen_between(
            field_text,
            tokens[end - 1].orig_end,
            tokens[end].orig_start,
        )
        && discharge_suffix_token(tokens[end].text.as_str())
    {
        if vaginal_context_token(tokens[end].text.as_str()) {
            saw_vaginal = true;
        }
        if nasal_context_token(tokens[end].text.as_str()) {
            saw_nasal = true;
        }
        end += 1;
    }

    if saw_vaginal {
        return Some((start, end, "271939006", "Vaginal discharge"));
    }
    if saw_nasal {
        return Some((start, end, "836474000", "Mucopurulent discharge"));
    }

    None
}

fn discharge_prefix_token(token: &str) -> bool {
    degree_modifier_token(token)
        || matches!(
            token,
            "thin"
                | "thick"
                | "grey"
                | "gray"
                | "white"
                | "greywhite"
                | "graywhite"
                | "homogeneous"
                | "mucopurulent"
                | "purulent"
                | "watery"
                | "vaginal"
                | "nasal"
        )
}

fn discharge_suffix_token(token: &str) -> bool {
    matches!(
        token,
        "coating"
            | "in"
            | "on"
            | "from"
            | "of"
            | "both"
            | "the"
            | "vaginal"
            | "walls"
            | "wall"
            | "nasal"
            | "passage"
            | "passages"
            | "nose"
            | "nostril"
            | "nostrils"
    )
}

fn vaginal_context_token(token: &str) -> bool {
    matches!(token, "vaginal" | "vagina" | "vulval" | "vulvar")
}

fn nasal_context_token(token: &str) -> bool {
    matches!(token, "nasal" | "nose" | "nostril" | "nostrils")
}

fn add_joint_effusion_matches(
    field: SoapField,
    field_text: &str,
    normalized: &NormalizedText,
    tokens: &[ExamToken],
    matches: &mut Vec<FindingMatch>,
) {
    for head_index in 0..tokens.len() {
        if !looks_like_effusion_token(tokens[head_index].text.as_str())
            || preceding_negation_token(tokens, head_index, 4)
            || non_joint_effusion_context(tokens, head_index)
            || !joint_effusion_context(tokens, head_index)
        {
            continue;
        }

        let mut start = head_index;
        let lower = head_index.saturating_sub(4);
        while start > lower
            && joint_effusion_prefix_token(tokens[start - 1].text.as_str())
            && !exam_phrase_boundary_between(
                field_text,
                tokens[start - 1].orig_end,
                tokens[start].orig_start,
            )
        {
            start -= 1;
        }

        add_structured_exam_match_from_tokens(
            field,
            field_text,
            normalized,
            tokens,
            StructuredExamMatchSpec {
                start_token: start,
                end_token: head_index + 1,
                concept_id: "387637008",
                preferred_term: "Effusion of joint",
                assertion: AssertionStatus::Affirmed,
                value: None,
            },
            matches,
        );
    }
}

fn looks_like_effusion_token(token: &str) -> bool {
    matches!(token, "effusion" | "effusions")
        || (token.len() == "effusion".len() + 1 && token.ends_with("effusion"))
}

fn joint_effusion_context(tokens: &[ExamToken], head_index: usize) -> bool {
    if head_index > 0 && matches!(tokens[head_index - 1].text.as_str(), "joint" | "joints") {
        return true;
    }
    preceding_musculoskeletal_exam_topic(tokens, head_index, 18)
}

fn non_joint_effusion_context(tokens: &[ExamToken], head_index: usize) -> bool {
    head_index > 0 && non_joint_effusion_modifier_token(tokens[head_index - 1].text.as_str())
}

fn non_joint_effusion_modifier_token(token: &str) -> bool {
    matches!(
        token,
        "ascitic" | "ear" | "middle" | "pericardial" | "peritoneal" | "pleural"
    )
}

fn joint_effusion_prefix_token(token: &str) -> bool {
    degree_modifier_token(token) || musculoskeletal_exam_topic_token(token)
}

fn add_muscle_weakness_matches(
    field: SoapField,
    field_text: &str,
    normalized: &NormalizedText,
    tokens: &[ExamToken],
    matches: &mut Vec<FindingMatch>,
) {
    for head_index in 0..tokens.len() {
        if !matches!(tokens[head_index].text.as_str(), "weak" | "weakness") {
            continue;
        }
        if preceding_negation_token(tokens, head_index, 5) {
            continue;
        }

        let lower = head_index.saturating_sub(5);
        let context = &tokens[lower..head_index];
        let concept = if context.iter().any(|token| {
            matches!(
                token.text.as_str(),
                "face" | "facial" | "forehead" | "nasolabial"
            )
        }) {
            Some(("95666008", "Weakness of face muscles"))
        } else if context
            .iter()
            .any(|token| muscle_weakness_context_token(token.text.as_str()))
        {
            Some(("26544005", "Muscle weakness"))
        } else {
            None
        };
        let Some((concept_id, preferred_term)) = concept else {
            continue;
        };

        let mut start = head_index;
        while start > lower
            && muscle_weakness_context_token(tokens[start - 1].text.as_str())
            && !exam_phrase_boundary_between(
                field_text,
                tokens[start - 1].orig_end,
                tokens[start].orig_start,
            )
        {
            start -= 1;
        }

        add_structured_exam_match_from_tokens(
            field,
            field_text,
            normalized,
            tokens,
            StructuredExamMatchSpec {
                start_token: start,
                end_token: head_index + 1,
                concept_id,
                preferred_term,
                assertion: AssertionStatus::Affirmed,
                value: None,
            },
            matches,
        );
    }
}

fn add_grip_limited_by_pain_matches(
    field: SoapField,
    field_text: &str,
    normalized: &NormalizedText,
    tokens: &[ExamToken],
    matches: &mut Vec<FindingMatch>,
) {
    for grip_index in 0..tokens.len() {
        if tokens[grip_index].text != "grip" {
            continue;
        }

        let scan_limit = (grip_index + 6).min(tokens.len());
        let mut limited_index = None;
        let mut pain_index = None;
        for index in grip_index + 1..scan_limit {
            if exam_phrase_boundary_between(
                field_text,
                tokens[index - 1].orig_end,
                tokens[index].orig_start,
            ) {
                break;
            }
            if matches!(
                tokens[index].text.as_str(),
                "limited" | "reduced" | "restricted"
            ) {
                limited_index = Some(index);
            }
            if matches!(tokens[index].text.as_str(), "pain" | "painful") {
                pain_index = Some(index);
                break;
            }
        }

        let (Some(_), Some(pain_index)) = (limited_index, pain_index) else {
            continue;
        };

        add_structured_exam_match_from_tokens(
            field,
            field_text,
            normalized,
            tokens,
            StructuredExamMatchSpec {
                start_token: grip_index,
                end_token: pain_index + 1,
                concept_id: "26544005",
                preferred_term: "Muscle weakness",
                assertion: AssertionStatus::Affirmed,
                value: None,
            },
            matches,
        );
    }
}

fn add_capillary_refill_threshold_matches(
    field: SoapField,
    field_text: &str,
    normalized: &NormalizedText,
    tokens: &[ExamToken],
    matches: &mut Vec<FindingMatch>,
) {
    for subject_start in 0..tokens.len() {
        let Some(subject_end) = match_any_phrase_at(
            tokens,
            subject_start,
            &["crt", "capillary refill", "capillary refill time"],
        ) else {
            continue;
        };

        let limit = (subject_end + 3).min(tokens.len());
        for value_index in subject_end..limit {
            let Some(gap) =
                field_text.get(tokens[subject_end - 1].orig_end..tokens[value_index].orig_end)
            else {
                continue;
            };
            if !gap.contains('<') || !looks_like_normal_crt_token(tokens[value_index].text.as_str())
            {
                continue;
            }

            add_structured_exam_match_from_tokens(
                field,
                field_text,
                normalized,
                tokens,
                StructuredExamMatchSpec {
                    start_token: subject_start,
                    end_token: value_index + 1,
                    concept_id: "45332005",
                    preferred_term: "Normal capillary filling",
                    assertion: AssertionStatus::Normal,
                    value: None,
                },
                matches,
            );
        }
    }
}

fn add_antalgic_gait_matches(
    field: SoapField,
    field_text: &str,
    normalized: &NormalizedText,
    tokens: &[ExamToken],
    matches: &mut Vec<FindingMatch>,
) {
    for gait_index in 0..tokens.len() {
        if tokens[gait_index].text != "gait" {
            continue;
        }

        let after_limit = (gait_index + 5).min(tokens.len());
        for antalgic_index in gait_index + 1..after_limit {
            if tokens[gait_index + 1..antalgic_index]
                .iter()
                .any(|token| !degree_modifier_token(token.text.as_str()))
            {
                break;
            }
            if tokens[antalgic_index].text != "antalgic" {
                continue;
            }
            add_structured_exam_match_from_tokens(
                field,
                field_text,
                normalized,
                tokens,
                StructuredExamMatchSpec {
                    start_token: gait_index,
                    end_token: antalgic_index + 1,
                    concept_id: "67141003",
                    preferred_term: "Antalgic gait",
                    assertion: AssertionStatus::Affirmed,
                    value: None,
                },
                matches,
            );
        }

        if gait_index == 0 {
            continue;
        }
        let mut start = gait_index - 1;
        while start > 0
            && degree_modifier_token(tokens[start - 1].text.as_str())
            && !exam_phrase_boundary_between(
                field_text,
                tokens[start - 1].orig_end,
                tokens[start].orig_start,
            )
        {
            start -= 1;
        }
        if tokens[start].text == "antalgic" {
            add_structured_exam_match_from_tokens(
                field,
                field_text,
                normalized,
                tokens,
                StructuredExamMatchSpec {
                    start_token: start,
                    end_token: gait_index + 1,
                    concept_id: "67141003",
                    preferred_term: "Antalgic gait",
                    assertion: AssertionStatus::Affirmed,
                    value: None,
                },
                matches,
            );
        }
    }
}

fn add_breast_lump_matches(
    field: SoapField,
    field_text: &str,
    normalized: &NormalizedText,
    tokens: &[ExamToken],
    matches: &mut Vec<FindingMatch>,
) {
    for head_index in 0..tokens.len() {
        if !matches!(tokens[head_index].text.as_str(), "lump" | "mass") {
            continue;
        }
        if preceding_negation_token(tokens, head_index, 5) {
            continue;
        }

        let lower = head_index.saturating_sub(12);
        let Some(site_index) = (lower..head_index).rev().find(|index| {
            matches!(tokens[*index].text.as_str(), "breast" | "breasts")
                && !breast_lump_boundary_between(
                    field_text,
                    tokens[*index].orig_end,
                    tokens[head_index].orig_start,
                )
        }) else {
            continue;
        };

        add_structured_exam_match_from_tokens(
            field,
            field_text,
            normalized,
            tokens,
            StructuredExamMatchSpec {
                start_token: site_index,
                end_token: head_index + 1,
                concept_id: "89164003",
                preferred_term: "Breast lump",
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
    if !should_add_structured_exam_match(
        matches, field, concept_id, span_start, span_end, assertion,
    ) {
        return;
    }
    matches.retain(|item| {
        !(item.field == field
            && item.concept_id == concept_id
            && !item
                .term_source
                .starts_with("built-in-structured-exam-feature")
            && span_start <= item.span_start
            && span_end >= item.span_end
            && ((span_start, span_end) != (item.span_start, item.span_end)
                || assertion != AssertionStatus::Affirmed))
    });

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
    assertion: AssertionStatus,
) -> bool {
    !matches.iter().any(|item| {
        let structured_replaces_raw = item.concept_id == concept_id
            && !item
                .term_source
                .starts_with("built-in-structured-exam-feature")
            && span_start <= item.span_start
            && span_end >= item.span_end
            && ((span_start, span_end) != (item.span_start, item.span_end)
                || assertion != AssertionStatus::Affirmed);
        item.field == field
            && ((item.concept_id == concept_id
                && spans_overlap(span_start, span_end, item.span_start, item.span_end)
                && !structured_replaces_raw)
                || (!item
                    .term_source
                    .starts_with("built-in-structured-exam-feature")
                    && spans_overlap(span_start, span_end, item.span_start, item.span_end)
                    && !structured_replaces_raw))
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

fn power_score_clause_limit(field_text: &str, tokens: &[ExamToken], start: usize) -> usize {
    let mut index = start;
    let hard_limit = (start + 18).min(tokens.len());
    while index < hard_limit {
        if index > start
            && exam_phrase_boundary_between(
                field_text,
                tokens[index - 1].orig_end,
                tokens[index].orig_start,
            )
        {
            break;
        }
        index += 1;
    }
    index
}

fn power_score_context_start(
    tokens: &[ExamToken],
    subject_end: usize,
    score_index: usize,
) -> usize {
    let lower = score_index.saturating_sub(3).max(subject_end);
    let mut start = score_index;
    while start > lower
        && matches!(
            tokens[start - 1].text.as_str(),
            "l" | "r"
                | "left"
                | "right"
                | "ehl"
                | "ankle"
                | "dorsiflexion"
                | "plantarflexion"
                | "knee"
                | "extension"
                | "biceps"
                | "wrist"
        )
    {
        start -= 1;
    }
    if start == score_index {
        score_index.saturating_sub(1).max(subject_end)
    } else {
        start
    }
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
    if matches!(token, "swelling" | "swollen" | "oedema" | "edema") {
        return true;
    }

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
            | "r"
            | "l"
            | "right"
            | "left"
            | "both"
            | "bilaterally"
            | "biceps"
            | "brachioradialis"
            | "coordination"
            | "dp"
            | "gait"
            | "hoffman"
            | "hoffmann"
            | "kernig"
            | "brudzinski"
            | "mild"
            | "mildly"
            | "moderate"
            | "moderately"
            | "kj"
            | "aj"
            | "power"
            | "pt"
            | "reflex"
            | "reflexes"
            | "range"
            | "movement"
            | "slight"
            | "slightly"
            | "tone"
            | "sensation"
            | "slr"
            | "straight"
            | "leg"
            | "raise"
            | "raising"
            | "s"
            | "sign"
            | "non"
            | "tender"
            | "nontender"
            | "test"
    )
}

fn preceding_musculoskeletal_exam_topic(
    tokens: &[ExamToken],
    index: usize,
    max_gap: usize,
) -> bool {
    let lower = index.saturating_sub(max_gap);
    tokens[lower..index]
        .iter()
        .any(|token| musculoskeletal_exam_topic_token(token.text.as_str()))
}

fn musculoskeletal_exam_topic_token(token: &str) -> bool {
    matches!(
        token,
        "ankle"
            | "arm"
            | "back"
            | "calf"
            | "cervical"
            | "elbow"
            | "finger"
            | "foot"
            | "hand"
            | "hip"
            | "joint"
            | "joints"
            | "knee"
            | "leg"
            | "lumbar"
            | "mcl"
            | "neck"
            | "patella"
            | "shoulder"
            | "spine"
            | "spinal"
            | "thoracic"
            | "toe"
            | "wrist"
    )
}

fn degree_modifier_token(token: &str) -> bool {
    matches!(
        token,
        "mild"
            | "mildly"
            | "moderate"
            | "moderately"
            | "severe"
            | "severely"
            | "slight"
            | "slightly"
            | "small"
            | "trace"
    )
}

fn muscle_weakness_context_token(token: &str) -> bool {
    matches!(
        token,
        "abduction"
            | "ankle"
            | "apb"
            | "biceps"
            | "dorsiflexion"
            | "ehl"
            | "external"
            | "face"
            | "facial"
            | "finger"
            | "grip"
            | "infraspinatus"
            | "knee"
            | "limb"
            | "muscle"
            | "plantarflexion"
            | "power"
            | "resisted"
            | "rotation"
            | "subscapularis"
            | "supraspinatus"
            | "thumb"
            | "wrist"
    )
}

fn preceding_negation_token(tokens: &[ExamToken], index: usize, max_gap: usize) -> bool {
    let lower = index.saturating_sub(max_gap);
    tokens[lower..index]
        .iter()
        .any(|token| exam_negation_token(token.text.as_str()))
}

fn looks_like_normal_crt_token(token: &str) -> bool {
    matches!(token, "2" | "2s" | "3" | "3s")
}

fn breast_lump_boundary_between(text: &str, start: usize, end: usize) -> bool {
    if start >= end || end > text.len() {
        return false;
    }
    text[start..end]
        .chars()
        .any(|ch| matches!(ch, ':' | ';' | '.' | '\n' | '\r' | '\u{2013}' | '\u{2014}'))
}

fn anatomical_exam_modifier_token(token: &str) -> bool {
    matches!(
        token,
        "abdominal"
            | "adnexal"
            | "adnexa"
            | "abdomen"
            | "angle"
            | "ankle"
            | "arm"
            | "back"
            | "breast"
            | "calf"
            | "cervical"
            | "chest"
            | "elbow"
            | "epicondyle"
            | "epigastric"
            | "extensor"
            | "fossa"
            | "foot"
            | "frontal"
            | "genital"
            | "hand"
            | "hip"
            | "iliac"
            | "joint"
            | "knee"
            | "lateral"
            | "left"
            | "line"
            | "lower"
            | "lumbar"
            | "mastoid"
            | "maxillary"
            | "medial"
            | "muscle"
            | "mucosa"
            | "mucosal"
            | "nasal"
            | "neck"
            | "oropharynx"
            | "origin"
            | "paraspinal"
            | "periorbital"
            | "renal"
            | "right"
            | "sacroiliac"
            | "scalp"
            | "skeletal"
            | "spinal"
            | "spine"
            | "sternum"
            | "sinus"
            | "sinuses"
            | "suprapubic"
            | "temporal"
            | "testicle"
            | "testis"
            | "thoracic"
            | "upper"
            | "uterine"
            | "uterus"
            | "vaginal"
            | "walls"
            | "wall"
            | "quadrant"
            | "wrist"
    )
}

fn post_tender_modifier_token(token: &str) -> bool {
    anatomical_exam_modifier_token(token) || matches!(token, "mcl" | "lcl")
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

fn exam_phrase_boundary_except_hyphen_between(text: &str, start: usize, end: usize) -> bool {
    if start >= end || end > text.len() {
        return false;
    }
    text[start..end]
        .chars()
        .any(|ch| matches!(ch, ':' | ';' | '.' | '\n' | '\r' | '\u{2013}' | '\u{2014}'))
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

    if matches!(extraction_kind, ExtractionKind::ExaminationFinding) {
        if raw.concept_id == "277233008"
            && is_bare_watery_discharge_match(&raw.normalized_match)
            && !has_nasal_context(raw.field, field_text, raw.span_start, raw.span_end)
        {
            return Some(crate::context::AssertionDecision {
                accepted: false,
                assertion: AssertionStatus::Ambiguous,
                rule_ids: vec!["CTX_EXAM_RHINORRHEA_WITHOUT_NASAL_CONTEXT".to_string()],
                explanation: "Suppressed: watery discharge wording is not specific to anterior rhinorrhea without nasal context."
                    .to_string(),
            });
        }

        if raw.concept_id == "15188001"
            && raw.normalized_match == "hearing"
            && has_normal_exam_status_after(raw.field, field_text, raw.span_end)
        {
            return Some(crate::context::AssertionDecision {
                accepted: false,
                assertion: AssertionStatus::Ambiguous,
                rule_ids: vec!["CTX_EXAM_HEARING_NORMAL_NOT_HEARING_LOSS".to_string()],
                explanation:
                    "Suppressed: hearing is documented as normal rather than hearing loss."
                        .to_string(),
            });
        }

        if raw.concept_id == "397540003"
            && raw.normalized_match == "vision"
            && has_normal_exam_status_after(raw.field, field_text, raw.span_end)
        {
            return Some(crate::context::AssertionDecision {
                accepted: false,
                assertion: AssertionStatus::Ambiguous,
                rule_ids: vec!["CTX_EXAM_VISION_NORMAL_NOT_VISUAL_IMPAIRMENT".to_string()],
                explanation: "Suppressed: vision is documented as normal rather than impaired."
                    .to_string(),
            });
        }

        if raw.concept_id == "397974008" && raw.normalized_match == "sensation" {
            return Some(crate::context::AssertionDecision {
                accepted: false,
                assertion: AssertionStatus::Ambiguous,
                rule_ids: vec!["CTX_EXAM_BARE_SENSATION_NOT_HYPOESTHESIA".to_string()],
                explanation:
                    "Suppressed: bare sensation requires reduced, impaired, or absent status to represent hypoaesthesia."
                        .to_string(),
            });
        }

        if raw.concept_id == "13791008"
            && raw.normalized_match == "weakness"
            && has_muscle_weakness_context(raw.field, field_text, raw.span_start, raw.span_end)
        {
            return Some(crate::context::AssertionDecision {
                accepted: false,
                assertion: AssertionStatus::Ambiguous,
                rule_ids: vec!["CTX_EXAM_LOCAL_WEAKNESS_NOT_ASTHENIA".to_string()],
                explanation:
                    "Suppressed: local examination weakness is muscle weakness, not general asthenia."
                        .to_string(),
            });
        }

        if post_status_sensitive_exam_concept(&raw.concept_id) {
            if let Some(assertion) = exam_status_after(raw.field, field_text, raw.span_end) {
                return Some(crate::context::AssertionDecision {
                    accepted: assertion == AssertionStatus::Affirmed,
                    assertion,
                    rule_ids: vec![structured_exam_rule_id(assertion).to_string()],
                    explanation: structured_exam_explanation(raw.field, assertion),
                });
            }
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

    if raw.concept_id == "397540003"
        && raw.normalized_match == "vision"
        && has_normal_exam_status_after(raw.field, field_text, raw.span_end)
    {
        return Some(crate::context::AssertionDecision {
            accepted: false,
            assertion: AssertionStatus::Ambiguous,
            rule_ids: vec!["CTX_EXAM_VISION_NORMAL_NOT_VISUAL_IMPAIRMENT".to_string()],
            explanation: "Suppressed: vision is documented as normal rather than impaired."
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

    if is_most_comfortable_listening_level_acronym(raw)
        && raw.value.is_none()
        && has_musculoskeletal_ligament_context(raw, field_text)
    {
        return Some(observable_ambiguous_decision(
            "CTX_OBSERVABLE_AUDIOLOGY_ACRONYM_IN_LIGAMENT_CONTEXT",
            "Suppressed: MCL appears in musculoskeletal ligament examination context, not as most comfortable listening level.",
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

fn is_most_comfortable_listening_level_acronym(raw: &RawMatch) -> bool {
    normalize_term(&raw.preferred_term) == "most comfortable listening level"
        && raw.normalized_match == "mcl"
}

fn has_musculoskeletal_ligament_context(raw: &RawMatch, field_text: &str) -> bool {
    let (window_start, window_end) = context_window(field_text, raw.span_start, raw.span_end, 120);
    let normalized = normalize_clinical_text(&field_text[window_start..window_end], raw.field).text;
    let tokens = normalized
        .split(' ')
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();

    let anatomical_context = tokens.iter().any(|token| {
        matches!(
            *token,
            "knee"
                | "joint"
                | "ligament"
                | "ligaments"
                | "medial"
                | "lateral"
                | "collateral"
                | "valgus"
                | "varus"
        )
    });
    let exam_context = tokens.iter().any(|token| {
        matches!(
            *token,
            "tender"
                | "tenderness"
                | "pain"
                | "painful"
                | "stress"
                | "stable"
                | "lax"
                | "laxity"
                | "effusion"
                | "swelling"
                | "line"
        )
    });

    anatomical_context && exam_context
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

fn is_bare_watery_discharge_match(normalized_match: &str) -> bool {
    matches!(normalized_match, "watery discharge" | "watery")
}

fn has_nasal_context(field: SoapField, text: &str, span_start: usize, span_end: usize) -> bool {
    let (window_start, window_end) = context_window(text, span_start, span_end, 120);
    let normalized = normalize_clinical_text(&text[window_start..window_end], field).text;
    let tokens = normalized
        .split(' ')
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();

    tokens.iter().any(|token| {
        matches!(
            *token,
            "nasal" | "nose" | "nostril" | "nostrils" | "rhinorrhea" | "rhinorrhoea"
        )
    })
}

fn has_normal_exam_status_after(field: SoapField, text: &str, span_end: usize) -> bool {
    let (_, window_end) = context_window(text, span_end, span_end, 40);
    let Some(after) = text.get(span_end..window_end) else {
        return false;
    };
    let normalized = normalize_clinical_text(after, field).text;
    normalized
        .split(' ')
        .take(4)
        .any(|token| matches!(token, "normal" | "intact" | "present" | "grossly"))
}

fn post_status_sensitive_exam_concept(concept_id: &str) -> bool {
    matches!(
        concept_id,
        "366448008"
            | "82668000"
            | "19411004"
            | "299391009"
            | "299393007"
            | "366605006"
            | "299849001"
            | "39051003"
            | "82345001"
            | "67672003"
    )
}

fn exam_status_after(field: SoapField, text: &str, span_end: usize) -> Option<AssertionStatus> {
    let (_, window_end) = context_window(text, span_end, span_end, 48);
    let after = text.get(span_end..window_end)?;
    let before_boundary = after
        .find(['.', ';', '\n', '\r'])
        .map(|idx| &after[..idx])
        .unwrap_or(after);
    let normalized = normalize_clinical_text(before_boundary, field).text;

    normalized
        .split(' ')
        .filter(|token| !token.is_empty())
        .take(6)
        .find_map(|token| match token {
            "positive" | "pos" => Some(AssertionStatus::Affirmed),
            "negative" | "neg" => Some(AssertionStatus::Negated),
            "normal" | "stable" | "intact" => Some(AssertionStatus::Normal),
            _ => None,
        })
}

fn has_muscle_weakness_context(
    field: SoapField,
    text: &str,
    span_start: usize,
    span_end: usize,
) -> bool {
    let (window_start, window_end) = context_window(text, span_start, span_end, 80);
    let normalized = normalize_clinical_text(&text[window_start..window_end], field).text;
    normalized
        .split(' ')
        .filter(|token| !token.is_empty())
        .any(muscle_weakness_context_token)
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

fn add_derived_body_site_matches(
    field: SoapField,
    field_text: &str,
    body_site_artefact: &TerminologyArtefact,
    body_site_matches: &mut Vec<RawMatch>,
) {
    let normalized = normalize_clinical_text(field_text, field);
    let tokens = exam_tokens(&normalized);

    add_abdominal_shorthand_body_site_matches(
        field,
        field_text,
        body_site_artefact,
        &tokens,
        body_site_matches,
    );
    add_mtp_body_site_matches(
        field,
        field_text,
        body_site_artefact,
        &tokens,
        body_site_matches,
    );
}

fn add_abdominal_shorthand_body_site_matches(
    field: SoapField,
    field_text: &str,
    body_site_artefact: &TerminologyArtefact,
    tokens: &[ExamToken],
    body_site_matches: &mut Vec<RawMatch>,
) {
    let Some((concept_id, preferred_term)) =
        body_site_concept_by_preferred_term(body_site_artefact, "Entire abdomen")
    else {
        return;
    };

    for (index, token) in tokens.iter().enumerate() {
        if token.text != "abdominal" {
            continue;
        }
        let Some(matched_text) = field_text.get(token.orig_start..token.orig_end) else {
            continue;
        };
        if !matched_text.eq_ignore_ascii_case("abdo") {
            continue;
        }
        push_derived_body_site_match(
            body_site_matches,
            RawMatch {
                concept_id: concept_id.to_string(),
                preferred_term: preferred_term.to_string(),
                field,
                span_start: tokens[index].orig_start,
                span_end: tokens[index].orig_end,
                matched_text: matched_text.to_string(),
                normalized_match: tokens[index].text.clone(),
                pattern_source: "built-in-body-site-shorthand".to_string(),
                value: None,
            },
        );
    }
}

fn add_mtp_body_site_matches(
    field: SoapField,
    field_text: &str,
    body_site_artefact: &TerminologyArtefact,
    tokens: &[ExamToken],
    body_site_matches: &mut Vec<RawMatch>,
) {
    for (index, token) in tokens.iter().enumerate() {
        if !metatarsophalangeal_joint_token(token.text.as_str()) {
            continue;
        }

        let hallux_context = has_first_mtp_context(tokens, index);
        let preferred_body_site = if hallux_context {
            "Hallux structure"
        } else {
            "Foot structure"
        };
        let Some((concept_id, preferred_term)) =
            body_site_concept_by_preferred_term(body_site_artefact, preferred_body_site)
        else {
            continue;
        };

        let mut start = if hallux_context && index > 0 {
            index - 1
        } else {
            index
        };
        if start > 0 && laterality_body_site_prefix(tokens[start - 1].text.as_str()) {
            start -= 1;
        }

        let span_start = tokens[start].orig_start;
        let span_end = tokens[index].orig_end;
        let Some(matched_text) = field_text.get(span_start..span_end) else {
            continue;
        };
        let normalized_match = tokens[start..=index]
            .iter()
            .map(|token| token.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        push_derived_body_site_match(
            body_site_matches,
            RawMatch {
                concept_id: concept_id.to_string(),
                preferred_term: preferred_term.to_string(),
                field,
                span_start,
                span_end,
                matched_text: matched_text.to_string(),
                normalized_match,
                pattern_source: "built-in-body-site-shorthand".to_string(),
                value: None,
            },
        );
    }
}

fn push_derived_body_site_match(body_site_matches: &mut Vec<RawMatch>, derived: RawMatch) {
    if body_site_matches.iter().any(|existing| {
        existing.field == derived.field
            && existing.concept_id == derived.concept_id
            && existing.span_start == derived.span_start
            && existing.span_end == derived.span_end
    }) {
        return;
    }

    body_site_matches.push(derived);
}

fn body_site_concept_by_preferred_term<'a>(
    body_site_artefact: &'a TerminologyArtefact,
    preferred_term: &str,
) -> Option<(&'a str, &'a str)> {
    body_site_artefact
        .concepts
        .iter()
        .find(|concept| concept.active && concept.preferred_term == preferred_term)
        .map(|concept| (concept.concept_id.as_str(), concept.preferred_term.as_str()))
}

fn metatarsophalangeal_joint_token(token: &str) -> bool {
    matches!(token, "mtp" | "mtpj" | "metatarsophalangeal")
}

fn has_first_mtp_context(tokens: &[ExamToken], index: usize) -> bool {
    index > 0
        && matches!(
            tokens[index - 1].text.as_str(),
            "first" | "1st" | "1" | "great" | "hallux"
        )
}

fn laterality_body_site_prefix(token: &str) -> bool {
    matches!(
        token,
        "r" | "l" | "right" | "left" | "bilateral" | "bilaterally" | "both"
    )
}

fn site_dependent_broad_finding(raw: &RawMatch) -> bool {
    let preferred = normalize_term(&raw.preferred_term);
    matches!(
        preferred.as_str(),
        "pain"
            | "itching"
            | "swelling"
            | "joint swelling"
            | "erythema"
            | "hot skin"
            | "tenderness"
            | "mass of body structure"
    ) || matches!(
        raw.normalized_match.as_str(),
        "pain"
            | "painful"
            | "itch"
            | "itching"
            | "itchy"
            | "swelling"
            | "swollen"
            | "red"
            | "redness"
            | "warm"
            | "warmth"
            | "hot"
            | "tender"
            | "tenderness"
            | "lump"
    )
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

fn topic_body_site_for_match(
    symptom: &RawMatch,
    field_text: &str,
    body_site_matches: &[RawMatch],
) -> Option<BodySiteMatch> {
    if !broad_symptom_can_use_topic_body_site(symptom) {
        return None;
    }

    body_site_matches
        .iter()
        .filter(|site| {
            site.field == symptom.field
                && !generic_body_site(site)
                && musculoskeletal_topic_body_site(site)
                && site.span_end <= symptom.span_start
                && symptom.span_start.saturating_sub(site.span_end) <= 360
                && body_site_topic_starts_scope(field_text, site.span_start, site.span_end)
                && body_site_topic_scope_reaches(field_text, site.span_end, symptom.span_start)
        })
        .max_by_key(|site| site.span_start)
        .map(body_site_match_from_raw)
}

fn broad_symptom_can_use_topic_body_site(symptom: &RawMatch) -> bool {
    let preferred = normalize_term(&symptom.preferred_term);
    matches!(
        preferred.as_str(),
        "pain"
            | "swelling"
            | "tenderness"
            | "erythema"
            | "hot skin"
            | "itching"
            | "mass of body structure"
    ) || matches!(
        symptom.normalized_match.as_str(),
        "pain" | "swelling" | "tender" | "tenderness" | "redness" | "warmth" | "lump" | "itch"
    )
}

fn musculoskeletal_topic_body_site(site: &RawMatch) -> bool {
    [site.preferred_term.as_str(), site.normalized_match.as_str()]
        .iter()
        .any(|text| {
            normalize_term(text).split(' ').any(|token| {
                matches!(
                    token,
                    "ankle"
                        | "arm"
                        | "back"
                        | "calf"
                        | "cervical"
                        | "elbow"
                        | "finger"
                        | "foot"
                        | "hand"
                        | "hallux"
                        | "hip"
                        | "joint"
                        | "knee"
                        | "leg"
                        | "lumbar"
                        | "neck"
                        | "patella"
                        | "shoulder"
                        | "spine"
                        | "spinal"
                        | "thoracic"
                        | "toe"
                        | "wrist"
                )
            })
        })
}

fn body_site_topic_starts_scope(field_text: &str, _site_start: usize, site_end: usize) -> bool {
    let Some(after) = field_text.get(site_end..) else {
        return false;
    };
    if heading_separator(after.trim_start()) {
        return true;
    }

    let after_end = (site_end + 80).min(field_text.len());
    let Some(window) = field_text.get(site_end..after_end) else {
        return false;
    };
    normalize_term(window).split(' ').any(|token| {
        matches!(
            token,
            "ache"
                | "aching"
                | "effusion"
                | "injury"
                | "locking"
                | "pain"
                | "rash"
                | "red"
                | "redness"
                | "stiffness"
                | "swelling"
                | "swollen"
                | "tender"
                | "tenderness"
                | "warm"
                | "warmth"
        )
    })
}

fn body_site_topic_scope_reaches(field_text: &str, site_end: usize, symptom_start: usize) -> bool {
    let Some(gap) = field_text.get(site_end..symptom_start) else {
        return false;
    };
    !gap.contains("\n\n") && !gap.contains("\r\n\r\n")
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
            | "across"
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
            | "area"
            | "areas"
            | "congested"
            | "congestion"
            | "hot"
            | "itchy"
            | "joint"
            | "lesion"
            | "lesions"
            | "mucosa"
            | "mucosal"
            | "patch"
            | "patches"
            | "rash"
            | "red"
            | "skin"
            | "swollen"
            | "tender"
            | "warm"
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
