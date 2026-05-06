use snomed_finding_extractor::rf2::build_from_openehr_valueset;
use snomed_finding_extractor::{Extractor, ObservableExtractRequest, SoapField};
use std::path::PathBuf;

#[test]
fn extracts_observable_entities_from_objective_only_payload() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("synthetic-observables.openehr-valueset.json");
    let artefact = build_from_openehr_valueset(path).unwrap();
    let extractor = Extractor::new(artefact).unwrap();

    let response = extractor
        .extract_observables(ObservableExtractRequest {
            note_id: Some("obs-1".to_string()),
            objective: "BP 128/82. HR 76. RR 14. Sats 98%. No temp recorded.".to_string(),
            include_suppressed: true,
            refset_id: Some("785380551000001102".to_string()),
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

    assert_eq!(response.note_id.as_deref(), Some("obs-1"));
    assert!(positives.contains(&("2000000001", SoapField::Objective, "BP")));
    assert!(positives.contains(&("2000000002", SoapField::Objective, "HR")));
    assert!(positives.contains(&("2000000003", SoapField::Objective, "RR")));
    assert!(positives.contains(&("2000000004", SoapField::Objective, "Sats")));
    assert!(response
        .matches
        .iter()
        .all(|item| item.rule_ids.len() == 1
            && item.rule_ids[0] == "ASSERT_AFFIRMED_PATIENT_OBSERVABLE"));
    assert!(suppressed
        .iter()
        .any(|item| item.0 == "2000000005" && item.1 == SoapField::Objective && item.2 == "temp"));
}

#[test]
fn observable_request_ignores_non_objective_soap_fields() {
    let request: snomed_finding_extractor::ExtractRequest = ObservableExtractRequest {
        note_id: Some("obs-2".to_string()),
        objective: "Heart rate 80.".to_string(),
        include_suppressed: false,
        refset_id: None,
    }
    .into();

    assert!(request.history.is_empty());
    assert_eq!(request.objective, "Heart rate 80.");
    assert!(request.assessment.is_empty());
    assert!(request.plan.is_empty());
}
