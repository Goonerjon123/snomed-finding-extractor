use snomed_finding_extractor::rf2::build_from_openehr_valueset;
use snomed_finding_extractor::{AssertionStatus, DiagnosisExtractRequest, Extractor, SoapField};
use std::path::PathBuf;

const DIAGNOSIS_REFSET_ID: &str = "782688301000001101";

fn diagnosis_extractor() -> Extractor {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("synthetic-diagnoses.openehr-valueset.json");
    let artefact = build_from_openehr_valueset(path).unwrap();
    Extractor::new(artefact).unwrap()
}

#[test]
fn extracts_diagnoses_from_assessment_only_payload() {
    let extractor = diagnosis_extractor();

    let response = extractor
        .extract_diagnoses(DiagnosisExtractRequest {
            note_id: Some("diag-1".to_string()),
            assessment: "Bronchial asthma. ?Pneumonia.".to_string(),
            include_suppressed: true,
            refset_id: Some(DIAGNOSIS_REFSET_ID.to_string()),
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
    let extractor = diagnosis_extractor();

    let response = extractor
        .extract_diagnoses(DiagnosisExtractRequest {
            note_id: Some("diag-3".to_string()),
            assessment: "Type 2 Diabetes. T2DM. URTI.".to_string(),
            include_suppressed: true,
            refset_id: Some(DIAGNOSIS_REFSET_ID.to_string()),
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

#[test]
fn accepts_clear_diagnosis_assessment_wording() {
    let extractor = diagnosis_extractor();

    let response = extractor
        .extract_diagnoses(DiagnosisExtractRequest {
            note_id: Some("diag-clear".to_string()),
            assessment: "Diagnosis: Bronchial asthma. Impression: Pneumonia. Type 2 Diabetes."
                .to_string(),
            include_suppressed: true,
            refset_id: Some(DIAGNOSIS_REFSET_ID.to_string()),
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

    assert!(positives.contains(&("4000000001", SoapField::Assessment, "Bronchial asthma")));
    assert!(positives.contains(&("4000000002", SoapField::Assessment, "Pneumonia")));
    assert!(positives.contains(&("4000000003", SoapField::Assessment, "Type 2 Diabetes")));
    assert!(response.suppressed.is_empty());
}

#[test]
fn suppresses_uncertain_assessment_diagnosis_wording() {
    let extractor = diagnosis_extractor();

    let response = extractor
        .extract_diagnoses(DiagnosisExtractRequest {
            note_id: Some("diag-uncertain-wording".to_string()),
            assessment: "Likely bronchial asthma. Working diagnosis: pneumonia. Treat as upper respiratory tract infection. Viral upper respiratory tract infection?"
                .to_string(),
            include_suppressed: true,
            refset_id: Some(DIAGNOSIS_REFSET_ID.to_string()),
        })
        .unwrap();

    assert!(response.matches.is_empty());
    for concept_id in ["4000000001", "4000000002", "4000000004", "4000000005"] {
        assert!(
            response.suppressed.iter().any(|item| {
                item.concept_id == concept_id
                    && item.assertion == AssertionStatus::Uncertain
                    && item
                        .rule_ids
                        .contains(&"CTX_DIAGNOSIS_UNCERTAIN_ASSESSMENT".to_string())
            }),
            "expected uncertain suppression for {concept_id}; got {:?}",
            response.suppressed
        );
    }
}

#[test]
fn suppresses_differential_and_alternative_diagnosis_lists() {
    let extractor = diagnosis_extractor();

    let response = extractor
        .extract_diagnoses(DiagnosisExtractRequest {
            note_id: Some("diag-differential".to_string()),
            assessment: "DDx: Bronchial asthma, Pneumonia. Type 2 diabetes mellitus or URTI."
                .to_string(),
            include_suppressed: true,
            refset_id: Some(DIAGNOSIS_REFSET_ID.to_string()),
        })
        .unwrap();

    assert!(response.matches.is_empty());
    for concept_id in ["4000000001", "4000000002"] {
        assert!(
            response.suppressed.iter().any(|item| {
                item.concept_id == concept_id
                    && item.assertion == AssertionStatus::Uncertain
                    && item
                        .rule_ids
                        .contains(&"CTX_DIAGNOSIS_DIFFERENTIAL_ASSESSMENT".to_string())
            }),
            "expected differential suppression for {concept_id}; got {:?}",
            response.suppressed
        );
    }
    for concept_id in ["4000000003", "4000000004"] {
        assert!(
            response.suppressed.iter().any(|item| {
                item.concept_id == concept_id
                    && item.assertion == AssertionStatus::Uncertain
                    && item
                        .rule_ids
                        .contains(&"CTX_DIAGNOSIS_ALTERNATIVE_ASSESSMENT".to_string())
            }),
            "expected alternative-list suppression for {concept_id}; got {:?}",
            response.suppressed
        );
    }
}

#[test]
fn negated_diagnosis_lists_remain_negated_not_uncertain() {
    let extractor = diagnosis_extractor();

    let response = extractor
        .extract_diagnoses(DiagnosisExtractRequest {
            note_id: Some("diag-negated-list".to_string()),
            assessment: "No bronchial asthma or pneumonia.".to_string(),
            include_suppressed: true,
            refset_id: Some(DIAGNOSIS_REFSET_ID.to_string()),
        })
        .unwrap();

    assert!(response.matches.is_empty());
    for concept_id in ["4000000001", "4000000002"] {
        assert!(
            response.suppressed.iter().any(|item| {
                item.concept_id == concept_id
                    && item.assertion == AssertionStatus::Negated
                    && item.rule_ids.contains(&"CTX_NEGATED_PRECEDING".to_string())
            }),
            "expected negated suppression for {concept_id}; got {:?}",
            response.suppressed
        );
    }
}
