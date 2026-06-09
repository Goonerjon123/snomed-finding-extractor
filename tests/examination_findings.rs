use snomed_finding_extractor::rf2::build_from_openehr_valueset;
use snomed_finding_extractor::{
    AssertionStatus, ExaminationFindingsExtractRequest, Extractor, SoapField,
};
use std::path::PathBuf;

#[test]
fn extracts_examination_findings_from_official_context_trimmed_variant() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("synthetic-examination-findings.openehr-valueset.json");
    let artefact = build_from_openehr_valueset(path).unwrap();
    let extractor = Extractor::new(artefact).unwrap();

    let response = extractor
        .extract_examination_findings(ExaminationFindingsExtractRequest {
            note_id: Some("exam-1".to_string()),
            objective: "Chest clear, no wheeze.".to_string(),
            include_suppressed: true,
            refset_id: Some("932266131000001101".to_string()),
        })
        .unwrap();

    let positives = response
        .matches
        .iter()
        .map(|item| {
            (
                item.concept_id.as_str(),
                item.field,
                item.matched_text.as_str(),
            )
        })
        .collect::<Vec<_>>();
    let suppressed = response
        .suppressed
        .iter()
        .map(|item| {
            (
                item.concept_id.as_str(),
                item.field,
                item.matched_text.as_str(),
                item.assertion,
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(response.note_id.as_deref(), Some("exam-1"));
    assert!(positives.contains(&("3000000001", SoapField::Objective, "Chest clear")));
    assert!(response.matches.iter().all(|item| item.rule_ids.len() == 1
        && item.rule_ids[0] == "ASSERT_AFFIRMED_PATIENT_EXAMINATION_FINDING"));
    assert!(suppressed.contains(&(
        "3000000002",
        SoapField::Objective,
        "wheeze",
        AssertionStatus::Negated
    )));
}

#[test]
fn examination_findings_request_ignores_non_objective_soap_fields() {
    let request: snomed_finding_extractor::ExtractRequest = ExaminationFindingsExtractRequest {
        note_id: Some("exam-2".to_string()),
        objective: "Lungs clear.".to_string(),
        include_suppressed: false,
        refset_id: None,
    }
    .into();

    assert!(request.history.is_empty());
    assert_eq!(request.objective, "Lungs clear.");
    assert!(request.assessment.is_empty());
    assert!(request.plan.is_empty());
}
