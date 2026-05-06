use crate::model::{ExtractRequest, SoapField};
use crate::terminology::TerminologyArtefact;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyntheticCase {
    pub id: String,
    pub request: ExtractRequest,
    pub expected_positive_concept_ids: Vec<String>,
    pub expected_suppressed_concept_ids: Vec<String>,
    pub scenario: SyntheticScenario,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyntheticScenario {
    AffirmedAssessment,
    NegatedHistory,
    UncertainHistory,
    FamilyHistory,
    HistoricalOrResolved,
    Conditional,
    Planned,
}

pub fn generate_synthetic_cases(
    artefact: &TerminologyArtefact,
    max_concepts: usize,
) -> Vec<SyntheticCase> {
    let mut cases = Vec::new();

    for concept in artefact
        .concepts
        .iter()
        .filter(|concept| concept.active)
        .take(max_concepts)
    {
        let term = concept.preferred_term.as_str();
        let safe_id_term = concept
            .concept_id
            .chars()
            .filter(|ch| ch.is_ascii_digit())
            .collect::<String>();

        cases.push(positive_case(
            format!("affirmed-assessment-{safe_id_term}"),
            format!("Likely {term}."),
            SoapField::Assessment,
            concept.concept_id.clone(),
            SyntheticScenario::AffirmedAssessment,
        ));
        cases.push(suppressed_case(
            format!("negated-history-{safe_id_term}"),
            format!("No {term} today."),
            SoapField::History,
            concept.concept_id.clone(),
            SyntheticScenario::NegatedHistory,
        ));
        cases.push(suppressed_case(
            format!("uncertain-history-{safe_id_term}"),
            format!("?{term} reported by patient."),
            SoapField::History,
            concept.concept_id.clone(),
            SyntheticScenario::UncertainHistory,
        ));
        cases.push(suppressed_case(
            format!("family-history-{safe_id_term}"),
            format!("Father had {term}."),
            SoapField::History,
            concept.concept_id.clone(),
            SyntheticScenario::FamilyHistory,
        ));
        cases.push(suppressed_case(
            format!("historical-{safe_id_term}"),
            format!("Past history of {term}, now resolved."),
            SoapField::History,
            concept.concept_id.clone(),
            SyntheticScenario::HistoricalOrResolved,
        ));
        cases.push(suppressed_case(
            format!("conditional-{safe_id_term}"),
            format!("If {term} develops, seek urgent review."),
            SoapField::Plan,
            concept.concept_id.clone(),
            SyntheticScenario::Conditional,
        ));
        cases.push(suppressed_case(
            format!("planned-{safe_id_term}"),
            format!("Screen for {term}."),
            SoapField::Plan,
            concept.concept_id.clone(),
            SyntheticScenario::Planned,
        ));
    }

    cases
}

fn positive_case(
    id: String,
    text: String,
    field: SoapField,
    concept_id: String,
    scenario: SyntheticScenario,
) -> SyntheticCase {
    let request = request_with_field(text, field, false);
    SyntheticCase {
        id,
        request,
        expected_positive_concept_ids: vec![concept_id],
        expected_suppressed_concept_ids: vec![],
        scenario,
    }
}

fn suppressed_case(
    id: String,
    text: String,
    field: SoapField,
    concept_id: String,
    scenario: SyntheticScenario,
) -> SyntheticCase {
    let request = request_with_field(text, field, true);
    SyntheticCase {
        id,
        request,
        expected_positive_concept_ids: vec![],
        expected_suppressed_concept_ids: vec![concept_id],
        scenario,
    }
}

fn request_with_field(text: String, field: SoapField, include_suppressed: bool) -> ExtractRequest {
    let mut request = ExtractRequest {
        include_suppressed,
        ..ExtractRequest::default()
    };
    match field {
        SoapField::History => request.history = text,
        SoapField::Objective => request.objective = text,
        SoapField::Assessment => request.assessment = text,
        SoapField::Plan => request.plan = text,
    }
    request
}
