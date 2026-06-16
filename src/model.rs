use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SoapField {
    History,
    Objective,
    Assessment,
    Plan,
}

impl SoapField {
    pub fn as_str(self) -> &'static str {
        match self {
            SoapField::History => "history",
            SoapField::Objective => "objective",
            SoapField::Assessment => "assessment",
            SoapField::Plan => "plan",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ExtractRequest {
    #[serde(default)]
    pub note_id: Option<String>,
    // The Subjective SOAP field is named `history` here; accept either key so
    // callers can post the literal SOAP field name.
    #[serde(default, alias = "subjective")]
    pub history: String,
    #[serde(default)]
    pub objective: String,
    #[serde(default)]
    pub assessment: String,
    #[serde(default)]
    pub plan: String,
    #[serde(default)]
    pub include_suppressed: bool,
    #[serde(default)]
    pub refset_id: Option<String>,
}

impl ExtractRequest {
    pub fn fields(&self) -> [(SoapField, &str); 4] {
        [
            (SoapField::History, self.history.as_str()),
            (SoapField::Objective, self.objective.as_str()),
            (SoapField::Assessment, self.assessment.as_str()),
            (SoapField::Plan, self.plan.as_str()),
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ObservableExtractRequest {
    #[serde(default)]
    pub note_id: Option<String>,
    #[serde(default)]
    pub objective: String,
    #[serde(default)]
    pub include_suppressed: bool,
    #[serde(default)]
    pub refset_id: Option<String>,
}

impl From<ObservableExtractRequest> for ExtractRequest {
    fn from(request: ObservableExtractRequest) -> Self {
        Self {
            note_id: request.note_id,
            objective: request.objective,
            include_suppressed: request.include_suppressed,
            refset_id: request.refset_id,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ExaminationFindingsExtractRequest {
    #[serde(default)]
    pub note_id: Option<String>,
    #[serde(default)]
    pub objective: String,
    #[serde(default)]
    pub include_suppressed: bool,
    #[serde(default)]
    pub refset_id: Option<String>,
}

impl From<ExaminationFindingsExtractRequest> for ExtractRequest {
    fn from(request: ExaminationFindingsExtractRequest) -> Self {
        Self {
            note_id: request.note_id,
            objective: request.objective,
            include_suppressed: request.include_suppressed,
            refset_id: request.refset_id,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct DiagnosisExtractRequest {
    #[serde(default)]
    pub note_id: Option<String>,
    #[serde(default)]
    pub assessment: String,
    #[serde(default)]
    pub include_suppressed: bool,
    #[serde(default)]
    pub refset_id: Option<String>,
}

impl From<DiagnosisExtractRequest> for ExtractRequest {
    fn from(request: DiagnosisExtractRequest) -> Self {
        Self {
            note_id: request.note_id,
            assessment: request.assessment,
            include_suppressed: request.include_suppressed,
            refset_id: request.refset_id,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtractResponse {
    pub note_id: Option<String>,
    pub matches: Vec<FindingMatch>,
    pub suppressed: Vec<SuppressedMatch>,
    pub terminology_version: String,
    pub engine_version: String,
    pub ruleset_version: String,
    pub artefact_hash: String,
    pub elapsed_micros: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FindingMatch {
    pub concept_id: String,
    pub preferred_term: String,
    pub field: SoapField,
    pub span_start: usize,
    pub span_end: usize,
    pub matched_text: String,
    pub normalized_match: String,
    /// How this term entered the artefact (e.g. "preferred_term",
    /// "openehr-description-acronym", "clinical_alias:...", "built-in-observable-alias").
    /// Replaces the former fixed `confidence` score, which was not a probability.
    pub term_source: String,
    /// Numeric value and unit captured immediately after an observable/numeric
    /// match ("BP 128/82" -> value "128/82"), so the EPR can populate an
    /// openEHR DV_QUANTITY without re-parsing the note. None for non-numeric
    /// matches or when no value follows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<MeasuredValue>,
    /// Optional SNOMED CT body site grouped with a broad symptom finding so
    /// downstream openEHR templates can populate symptom code and body site.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_site: Option<BodySiteMatch>,
    /// Assertion state of the matched evidence. Affirmed matches omit this in
    /// JSON for backward compatibility; non-affirmed exam findings include it
    /// so downstream consumers do not mistake absence for presence.
    #[serde(default, skip_serializing_if = "is_affirmed_assertion")]
    pub assertion: AssertionStatus,
    pub rule_ids: Vec<String>,
    pub explanation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BodySiteMatch {
    pub concept_id: String,
    pub preferred_term: String,
    pub span_start: usize,
    pub span_end: usize,
    pub matched_text: String,
    pub normalized_match: String,
    pub term_source: String,
}

/// A measurement value captured from the text after a numeric match.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MeasuredValue {
    /// Raw value text as typed, e.g. "128/82", "37.8", "98".
    pub text: String,
    /// Unit token if one immediately follows the value, e.g. "%", "kg", "mmHg".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Original-text span of the captured value (and unit when present).
    pub span_start: usize,
    pub span_end: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SuppressedMatch {
    pub concept_id: String,
    pub preferred_term: String,
    pub field: SoapField,
    pub span_start: usize,
    pub span_end: usize,
    pub matched_text: String,
    pub normalized_match: String,
    pub assertion: AssertionStatus,
    pub rule_ids: Vec<String>,
    pub explanation: String,
}

#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq, Hash, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum AssertionStatus {
    #[default]
    Affirmed,
    Normal,
    Negated,
    Uncertain,
    FamilyHistory,
    HistoricalOrResolved,
    Hypothetical,
    Conditional,
    Planned,
    NonPatient,
    Ambiguous,
}

fn is_affirmed_assertion(assertion: &AssertionStatus) -> bool {
    *assertion == AssertionStatus::Affirmed
}
