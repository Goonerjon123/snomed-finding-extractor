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

    // The captured value/unit lets the EPR fill an openEHR quantity without
    // re-parsing the note.
    let bp = response
        .matches
        .iter()
        .find(|item| item.concept_id == "2000000001")
        .expect("BP match present");
    let value = bp.value.as_ref().expect("BP value captured");
    assert_eq!(value.text, "128/82");
    assert_eq!(bp.matched_text, "BP");

    let sats = response
        .matches
        .iter()
        .find(|item| item.concept_id == "2000000004")
        .expect("Sats match present");
    let sats_value = sats.value.as_ref().expect("Sats value captured");
    assert_eq!(sats_value.text, "98");
    assert_eq!(sats_value.unit.as_deref(), Some("%"));
}

#[test]
fn captures_values_through_filler_words() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("synthetic-observables.openehr-valueset.json");
    let artefact = build_from_openehr_valueset(path).unwrap();
    let extractor = Extractor::new(artefact).unwrap();

    let response = extractor
        .extract_observables(ObservableExtractRequest {
            note_id: Some("obs-filler".to_string()),
            objective: "BP today 148/92. HR of 88 bpm.".to_string(),
            include_suppressed: false,
            refset_id: Some("785380551000001102".to_string()),
        })
        .unwrap();

    let bp = response
        .matches
        .iter()
        .find(|item| item.concept_id == "2000000001")
        .expect("BP match present despite filler word");
    assert_eq!(bp.value.as_ref().unwrap().text, "148/92");

    let hr = response
        .matches
        .iter()
        .find(|item| item.concept_id == "2000000002")
        .expect("HR match present");
    let hr_value = hr.value.as_ref().unwrap();
    assert_eq!(hr_value.text, "88");
    assert_eq!(hr_value.unit.as_deref(), Some("bpm"));
}

#[test]
fn captures_values_from_compact_gp_copd_objective_vitals() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("synthetic-observables.openehr-valueset.json");
    let artefact = build_from_openehr_valueset(path).unwrap();
    let extractor = Extractor::new(artefact).unwrap();

    let response = extractor
        .extract_observables(ObservableExtractRequest {
            note_id: Some("obs-copd".to_string()),
            objective: "Mildly SOB at rest, talking in full sentences. Sats 93% RA (baseline 94%), RR 22, afeb 37.2, HR 92. Chest: widespread wheeze + coarse creps R base."
                .to_string(),
            include_suppressed: true,
            refset_id: Some("785380551000001102".to_string()),
        })
        .unwrap();

    let sats = response
        .matches
        .iter()
        .find(|item| item.concept_id == "2000000004")
        .expect("oxygen saturation match present");
    assert_eq!(sats.matched_text, "Sats");
    let sats_value = sats.value.as_ref().expect("Sats value captured");
    assert_eq!(sats_value.text, "93");
    assert_eq!(sats_value.unit.as_deref(), Some("%"));

    let rr = response
        .matches
        .iter()
        .find(|item| item.concept_id == "2000000003")
        .expect("respiratory rate match present");
    assert_eq!(rr.matched_text, "RR");
    assert_eq!(rr.value.as_ref().expect("RR value captured").text, "22");

    let temp = response
        .matches
        .iter()
        .find(|item| item.concept_id == "2000000005")
        .expect("afebrile temperature match present");
    assert_eq!(temp.matched_text, "afeb");
    assert_eq!(
        temp.value.as_ref().expect("afeb value captured").text,
        "37.2"
    );

    let hr = response
        .matches
        .iter()
        .find(|item| item.concept_id == "2000000002")
        .expect("heart rate match present");
    assert_eq!(hr.matched_text, "HR");
    assert_eq!(hr.value.as_ref().expect("HR value captured").text, "92");
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

#[test]
fn extracts_numeric_observable_labels_only_before_values() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("synthetic-observables.openehr-valueset.json");
    let artefact = build_from_openehr_valueset(path).unwrap();
    let extractor = Extractor::new(artefact).unwrap();

    let response = extractor
        .extract_observables(ObservableExtractRequest {
            note_id: Some("obs-3".to_string()),
            objective: "T: 37.8. P: 96. Pulse 96. PR: 96. T waves normal. P waves normal."
                .to_string(),
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

    assert!(positives.contains(&("2000000005", SoapField::Objective, "T")));
    assert!(positives.contains(&("2000000006", SoapField::Objective, "P")));
    assert!(positives.contains(&("2000000006", SoapField::Objective, "Pulse")));
    assert!(positives.contains(&("2000000006", SoapField::Objective, "PR")));
    assert_eq!(
        positives
            .iter()
            .filter(|item| **item == ("2000000005", SoapField::Objective, "T"))
            .count(),
        1
    );
    assert_eq!(
        positives
            .iter()
            .filter(|item| **item == ("2000000006", SoapField::Objective, "P"))
            .count(),
        1
    );
}
