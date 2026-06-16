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
                item.assertion,
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(response.note_id.as_deref(), Some("exam-1"));
    assert!(positives.contains(&(
        "3000000001",
        SoapField::Objective,
        "Chest clear",
        AssertionStatus::Affirmed
    )));
    assert!(positives.contains(&(
        "3000000002",
        SoapField::Objective,
        "wheeze",
        AssertionStatus::Negated
    )));
    assert!(response.matches.iter().any(|item| item.rule_ids.len() == 1
        && item.rule_ids[0] == "ASSERT_AFFIRMED_PATIENT_EXAMINATION_FINDING"));
    assert!(response
        .matches
        .iter()
        .any(|item| item.rule_ids.len() == 1 && item.rule_ids[0] == "CTX_NEGATED_PRECEDING"));
    assert!(response.suppressed.is_empty());
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

#[test]
fn extracts_body_site_signs_with_intervening_modifiers() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("synthetic-examination-findings.openehr-valueset.json");
    let artefact = build_from_openehr_valueset(path).unwrap();
    let extractor = Extractor::new(artefact).unwrap();

    let response = extractor
        .extract_examination_findings(ExaminationFindingsExtractRequest {
            note_id: Some("exam-3".to_string()),
            objective: "Exudate on swollen left tonsil.".to_string(),
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

    assert!(positives.contains(&(
        "3000000003",
        SoapField::Objective,
        "Exudate on swollen left tonsil"
    )));
    assert!(positives.contains(&("3000000004", SoapField::Objective, "swollen left tonsil")));
}

#[test]
fn examination_findings_include_negated_findings_without_suppressed_flag() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("synthetic-examination-findings.openehr-valueset.json");
    let artefact = build_from_openehr_valueset(path).unwrap();
    let extractor = Extractor::new(artefact).unwrap();

    let response = extractor
        .extract_examination_findings(ExaminationFindingsExtractRequest {
            note_id: Some("exam-4".to_string()),
            objective: "No wheeze.".to_string(),
            include_suppressed: false,
            refset_id: Some("932266131000001101".to_string()),
        })
        .unwrap();

    assert_eq!(response.matches.len(), 1);
    assert_eq!(response.matches[0].concept_id, "3000000002");
    assert_eq!(response.matches[0].matched_text, "wheeze");
    assert_eq!(response.matches[0].assertion, AssertionStatus::Negated);
    assert!(response.suppressed.is_empty());
}

#[test]
fn examination_findings_include_normal_exam_feature_statements() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("synthetic-examination-findings.openehr-valueset.json");
    let artefact = build_from_openehr_valueset(path).unwrap();
    let extractor = Extractor::new(artefact).unwrap();

    let response = extractor
        .extract_examination_findings(ExaminationFindingsExtractRequest {
            note_id: Some("exam-5".to_string()),
            objective: "HS normal. Lungs clear. Abdo SNT. Chest clear.".to_string(),
            include_suppressed: false,
            refset_id: Some("932266131000001101".to_string()),
        })
        .unwrap();

    let matches = response
        .matches
        .iter()
        .map(|item| {
            (
                item.concept_id.as_str(),
                item.preferred_term.as_str(),
                item.matched_text.as_str(),
                item.assertion,
                item.term_source.as_str(),
            )
        })
        .collect::<Vec<_>>();

    assert!(matches.contains(&(
        "271660002",
        "Heart sounds",
        "HS normal",
        AssertionStatus::Normal,
        "built-in-normal-exam-feature"
    )));
    assert!(matches.contains(&(
        "364060002",
        "Chest auscultation feature",
        "Lungs clear",
        AssertionStatus::Normal,
        "built-in-normal-exam-feature"
    )));
    assert!(matches.contains(&(
        "271911005",
        "Abdominal examination finding",
        "Abdo SNT",
        AssertionStatus::Normal,
        "built-in-normal-exam-feature"
    )));
    assert!(response.matches.iter().any(|item| {
        item.concept_id == "3000000001"
            && item.matched_text == "Chest clear"
            && item.assertion == AssertionStatus::Affirmed
    }));
    assert_eq!(
        response
            .matches
            .iter()
            .filter(|item| item.matched_text == "Chest clear")
            .count(),
        1
    );
}
