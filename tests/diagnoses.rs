use snomed_finding_extractor::rf2::build_from_openehr_valueset;
use snomed_finding_extractor::{AssertionStatus, DiagnosisExtractRequest, Extractor, SoapField};
use std::path::PathBuf;

#[test]
fn extracts_diagnoses_from_assessment_only_payload() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("synthetic-diagnoses.openehr-valueset.json");
    let artefact = build_from_openehr_valueset(path).unwrap();
    let extractor = Extractor::new(artefact).unwrap();

    let response = extractor
        .extract_diagnoses(DiagnosisExtractRequest {
            note_id: Some("diag-1".to_string()),
            assessment: "Bronchial asthma. ?Pneumonia.".to_string(),
            include_suppressed: true,
            refset_id: Some("782688301000001101".to_string()),
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

    assert_eq!(response.note_id.as_deref(), Some("diag-1"));
    assert!(positives.contains(&("4000000001", SoapField::Assessment, "Bronchial asthma")));
    assert!(response
        .matches
        .iter()
        .all(|item| item.rule_ids.len() == 1
            && item.rule_ids[0] == "ASSERT_AFFIRMED_PATIENT_DIAGNOSIS"));
    assert!(suppressed.contains(&(
        "4000000002",
        SoapField::Assessment,
        "Pneumonia",
        AssertionStatus::Uncertain
    )));
}

#[test]
fn diagnosis_request_ignores_non_assessment_soap_fields() {
    let request: snomed_finding_extractor::ExtractRequest = DiagnosisExtractRequest {
        note_id: Some("diag-2".to_string()),
        assessment: "Asthma.".to_string(),
        include_suppressed: false,
        refset_id: None,
    }
    .into();

    assert!(request.history.is_empty());
    assert!(request.objective.is_empty());
    assert_eq!(request.assessment, "Asthma.");
    assert!(request.plan.is_empty());
}

#[test]
fn extracts_diagnoses_from_official_description_variants() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("synthetic-diagnoses.openehr-valueset.json");
    let artefact = build_from_openehr_valueset(path).unwrap();
    let extractor = Extractor::new(artefact).unwrap();

    let response = extractor
        .extract_diagnoses(DiagnosisExtractRequest {
            note_id: Some("diag-3".to_string()),
            assessment: "Type 2 Diabetes. T2DM. URTI.".to_string(),
            include_suppressed: true,
            refset_id: Some("782688301000001101".to_string()),
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

    assert!(positives.contains(&("4000000003", SoapField::Assessment, "Type 2 Diabetes")));
    assert!(positives.contains(&("4000000003", SoapField::Assessment, "T2DM")));
    assert!(positives.contains(&("4000000004", SoapField::Assessment, "URTI")));
    assert!(!positives.contains(&("4000000005", SoapField::Assessment, "URTI")));
}
