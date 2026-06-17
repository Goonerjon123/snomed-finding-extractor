use snomed_finding_extractor::rf2::build_from_openehr_valueset;
use snomed_finding_extractor::terminology::{ConceptEntry, TermVariant};
use snomed_finding_extractor::{
    AssertionStatus, Extractor, ObservableExtractRequest, SoapField, TerminologyArtefact,
};
use std::path::PathBuf;

fn observable_extractor_with(concepts: Vec<ConceptEntry>) -> Extractor {
    Extractor::new(TerminologyArtefact {
        schema_version: 1,
        terminology_version: "fixture-observables".to_string(),
        source_release: "fixture".to_string(),
        refset_id: "fixture-observables".to_string(),
        generated_at_utc: "fixture".to_string(),
        concepts,
        artefact_hash: String::new(),
    })
    .unwrap()
}

fn observable_concept(
    concept_id: &str,
    preferred_term: &str,
    variants: &[(&str, bool)],
) -> ConceptEntry {
    ConceptEntry {
        concept_id: concept_id.to_string(),
        active: true,
        preferred_term: preferred_term.to_string(),
        descriptions: vec![],
        variants: variants
            .iter()
            .map(|(text, requires_numeric_value)| TermVariant {
                text: text.to_string(),
                source: if *requires_numeric_value {
                    "openehr-observable-numeric-label".to_string()
                } else {
                    "fixture".to_string()
                },
                description_id: None,
                allow_ambiguous: *requires_numeric_value,
                requires_numeric_value: *requires_numeric_value,
            })
            .collect(),
    }
}

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
fn captures_blood_pressure_written_with_over_separator() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("synthetic-observables.openehr-valueset.json");
    let artefact = build_from_openehr_valueset(path).unwrap();
    let extractor = Extractor::new(artefact).unwrap();

    let response = extractor
        .extract_observables(ObservableExtractRequest {
            note_id: Some("obs-bp-over".to_string()),
            objective: "BP 146 over 86. Pulse 92 regular.".to_string(),
            include_suppressed: false,
            refset_id: Some("785380551000001102".to_string()),
        })
        .unwrap();

    let bp = response
        .matches
        .iter()
        .find(|item| item.preferred_term == "Blood pressure")
        .expect("BP match present");
    assert_eq!(bp.value.as_ref().unwrap().text, "146/86");
    assert_eq!(bp.value.as_ref().unwrap().span_start, 3);
    assert_eq!(bp.value.as_ref().unwrap().span_end, 14);
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

#[test]
fn suppresses_peripheral_vascular_exam_observable_false_positives() {
    let extractor = observable_extractor_with(vec![
        observable_concept("8499008", "Pulse", &[("pulse", false)]),
        observable_concept("373713005", "Sensory perception", &[("sensation", false)]),
        observable_concept("86290005", "Respiratory rate", &[("R", true)]),
        observable_concept(
            "404980009",
            "Spine - range of movement",
            &[("range of movement", false)],
        ),
        observable_concept("9964006", "Flexion", &[("flexion", false)]),
        observable_concept("63448001", "Gait", &[("gait", false)]),
        observable_concept("87572000", "Reflex", &[("reflex", false)]),
        observable_concept("85352007", "Coordination", &[("coordination", false)]),
        observable_concept(
            "364417002",
            "Movement of neck",
            &[("movement of neck", false)],
        ),
        observable_concept(
            "250892005",
            "Most comfortable listening level",
            &[("MCL", false)],
        ),
    ]);

    let first = extractor
        .extract_observables(ObservableExtractRequest {
            objective: "BM 9.8. Feet: skin intact, no ulcers/callus/deformity. DP + PT pulses palpable bilaterally. Monofilament - sensation reduced, absent at 4/10 sites R, 3/10 L. Vibration reduced to ankles.".to_string(),
            include_suppressed: true,
            refset_id: Some("fixture-observables".to_string()),
            ..ObservableExtractRequest::default()
        })
        .unwrap();

    assert!(first.matches.is_empty());
    assert!(first.suppressed.iter().any(|item| {
        item.concept_id == "8499008"
            && item.matched_text == "pulses"
            && item.assertion == AssertionStatus::Ambiguous
            && item
                .rule_ids
                .contains(&"CTX_OBSERVABLE_PULSE_WITHOUT_NUMERIC_VALUE".to_string())
    }));
    assert!(first.suppressed.iter().any(|item| {
        item.concept_id == "373713005"
            && item.matched_text == "sensation"
            && item.assertion == AssertionStatus::Ambiguous
            && item
                .rule_ids
                .contains(&"CTX_OBSERVABLE_SENSATION_IN_EXAM_CONTEXT".to_string())
    }));
    assert!(first.suppressed.iter().any(|item| {
        item.concept_id == "86290005"
            && item.matched_text == "R"
            && item.assertion == AssertionStatus::Ambiguous
            && item
                .rule_ids
                .contains(&"CTX_OBSERVABLE_RESP_RATE_SIDE_LABEL".to_string())
    }));

    let second = extractor
        .extract_observables(ObservableExtractRequest {
            objective: "Distal pulses + sensation intact, CRT brisk. Reflexes symmetrical. Coordination + gait normal. Neck - paraspinal tenderness, full ROM. Antalgic gait. Flexion reduced. Pulse 96. R 18."
                .to_string(),
            include_suppressed: true,
            refset_id: Some("fixture-observables".to_string()),
            ..ObservableExtractRequest::default()
        })
        .unwrap();

    let positives = second
        .matches
        .iter()
        .map(|item| (item.concept_id.as_str(), item.matched_text.as_str()))
        .collect::<Vec<_>>();
    assert!(positives.contains(&("8499008", "Pulse")));
    assert!(positives.contains(&("86290005", "R")));
    assert!(!positives.contains(&("8499008", "pulses")));
    assert!(!positives.contains(&("373713005", "sensation")));
    assert!(!positives.contains(&("404980009", "ROM")));
    assert!(!positives.contains(&("9964006", "Flexion")));
    assert!(!positives.contains(&("63448001", "gait")));
    assert!(!positives.contains(&("87572000", "Reflexes")));
    assert!(!positives.contains(&("85352007", "Coordination")));
    assert!(!positives.contains(&("364417002", "Neck - paraspinal tenderness, full ROM")));
    assert!(!positives.contains(&("250892005", "MCL")));
    for concept_id in [
        "404980009",
        "9964006",
        "63448001",
        "87572000",
        "85352007",
        "364417002",
    ] {
        assert!(
            second.suppressed.iter().any(|item| {
                item.concept_id == concept_id
                    && item.assertion == AssertionStatus::Ambiguous
                    && item
                        .rule_ids
                        .contains(&"CTX_OBSERVABLE_QUALITATIVE_EXAM_CONTEXT".to_string())
            }),
            "expected qualitative exam suppression for {concept_id}"
        );
    }

    let knee = extractor
        .extract_observables(ObservableExtractRequest {
            objective: "R knee - moderate effusion. Tender medial joint line + over MCL. Valgus stress - pain medially but stable.".to_string(),
            include_suppressed: true,
            refset_id: Some("fixture-observables".to_string()),
            ..ObservableExtractRequest::default()
        })
        .unwrap();

    assert!(knee.matches.is_empty());
    assert!(knee.suppressed.iter().any(|item| {
        item.concept_id == "250892005"
            && item.matched_text == "MCL"
            && item.assertion == AssertionStatus::Ambiguous
            && item
                .rule_ids
                .contains(&"CTX_OBSERVABLE_AUDIOLOGY_ACRONYM_IN_LIGAMENT_CONTEXT".to_string())
    }));
}
