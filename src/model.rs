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
    #[serde(default)]
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
    pub confidence: f32,
    pub rule_ids: Vec<String>,
    pub explanation: String,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum AssertionStatus {
    Affirmed,
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
