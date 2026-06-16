use snomed_finding_extractor::rf2::build_from_openehr_valueset;
use snomed_finding_extractor::terminology::{ConceptEntry, TermVariant, TerminologyArtefact};
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
        "built-in-structured-exam-feature"
    )));
    assert!(matches.contains(&(
        "364060002",
        "Chest auscultation feature",
        "Lungs clear",
        AssertionStatus::Normal,
        "built-in-structured-exam-feature"
    )));
    assert!(matches.contains(&(
        "271911005",
        "Abdominal examination finding",
        "Abdo SNT",
        AssertionStatus::Normal,
        "built-in-structured-exam-feature"
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

#[test]
fn examination_findings_include_structured_normal_and_negative_exam_statements() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("synthetic-examination-findings.openehr-valueset.json");
    let artefact = build_from_openehr_valueset(path).unwrap();
    let extractor = Extractor::new(artefact).unwrap();

    let response = extractor
        .extract_examination_findings(ExaminationFindingsExtractRequest {
            note_id: Some("exam-6".to_string()),
            objective: "Alert + orientated. Neuro - CN II-XII intact, fundi normal (no papilloedema), power 5/5, reflexes symmetrical, coordination + gait normal. Neck - paraspinal tenderness, full ROM. Temporal arteries non-tender + pulsatile, no scalp tenderness.".to_string(),
            include_suppressed: true,
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
            )
        })
        .collect::<Vec<_>>();

    for expected in [
        (
            "248233002",
            "Mental alertness",
            "Alert",
            AssertionStatus::Normal,
        ),
        (
            "43173001",
            "Orientation",
            "orientated",
            AssertionStatus::Normal,
        ),
        (
            "246569003",
            "Function of specific cranial nerves",
            "CN II-XII intact",
            AssertionStatus::Normal,
        ),
        (
            "164734008",
            "Fundoscopy normal",
            "fundi normal",
            AssertionStatus::Normal,
        ),
        (
            "423488006",
            "Papilledema - optic disc edema due to raised intracranial pressure",
            "no papilloedema",
            AssertionStatus::Negated,
        ),
        (
            "249948009",
            "Grade of muscle power",
            "power 5/5",
            AssertionStatus::Normal,
        ),
        (
            "246581004",
            "Peripheral reflex",
            "reflexes symmetrical",
            AssertionStatus::Normal,
        ),
        (
            "363844006",
            "Pattern of coordination",
            "coordination + gait normal",
            AssertionStatus::Normal,
        ),
        ("63448001", "Gait", "gait normal", AssertionStatus::Normal),
        (
            "301399007",
            "Musculoskeletal tenderness",
            "paraspinal tenderness",
            AssertionStatus::Affirmed,
        ),
        (
            "404980009",
            "Spine - range of movement",
            "full ROM",
            AssertionStatus::Normal,
        ),
        (
            "301399007",
            "Musculoskeletal tenderness",
            "Temporal arteries non-tender",
            AssertionStatus::Negated,
        ),
        (
            "422176008",
            "Temporal pulse, function",
            "Temporal arteries non-tender + pulsatile",
            AssertionStatus::Normal,
        ),
        (
            "301399007",
            "Musculoskeletal tenderness",
            "no scalp tenderness",
            AssertionStatus::Negated,
        ),
    ] {
        assert!(
            matches.contains(&expected),
            "expected structured exam match {expected:?}"
        );
    }

    let power = response
        .matches
        .iter()
        .find(|item| item.concept_id == "249948009")
        .expect("power score match");
    assert_eq!(power.value.as_ref().unwrap().text, "5/5");
    assert!(response.suppressed.is_empty());
}

#[test]
fn examination_findings_include_common_status_lists_and_named_tests() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("synthetic-examination-findings.openehr-valueset.json");
    let artefact = build_from_openehr_valueset(path).unwrap();
    let extractor = Extractor::new(artefact).unwrap();

    let response = extractor
        .extract_examination_findings(ExaminationFindingsExtractRequest {
            note_id: Some("exam-7".to_string()),
            objective: "Power: L EHL 4/5, ankle dorsiflexion 4/5. Reflexes: KJ symmetrical, AJ reduced L. 10g monofilament sensation absent. Vibration reduced. Proprioception reduced. DP + PT absent R. CRT <3s. SLR positive L, crossed SLR negative. Spurling's test positive. Kernig + Brudzinski negative. Hoffman's negative.".to_string(),
            include_suppressed: true,
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
                item.assertion,
            )
        })
        .collect::<Vec<_>>();

    for expected in [
        (
            "249948009",
            "Grade of muscle power",
            AssertionStatus::Affirmed,
        ),
        ("835279003", "Decreased reflex", AssertionStatus::Affirmed),
        (
            "390932001",
            "10g monofilament sensation absent",
            AssertionStatus::Affirmed,
        ),
        (
            "299934008",
            "Impaired vibration sensation",
            AssertionStatus::Affirmed,
        ),
        (
            "103003004",
            "Impaired body position sense",
            AssertionStatus::Affirmed,
        ),
        (
            "301170006",
            "Dorsalis pulse absent",
            AssertionStatus::Affirmed,
        ),
        (
            "301169005",
            "Posterior tibial pulse absent",
            AssertionStatus::Affirmed,
        ),
        (
            "45332005",
            "Normal capillary filling",
            AssertionStatus::Normal,
        ),
        (
            "366448008",
            "Finding of straight leg raise",
            AssertionStatus::Affirmed,
        ),
        (
            "82668000",
            "Crossed leg raising sign",
            AssertionStatus::Negated,
        ),
        ("19411004", "Spurling sign", AssertionStatus::Affirmed),
        ("39051003", "Kernig's sign", AssertionStatus::Negated),
        ("82345001", "Brudzinski's sign", AssertionStatus::Negated),
        (
            "299849001",
            "Hoffman's reflex positive",
            AssertionStatus::Negated,
        ),
    ] {
        assert!(
            matches.contains(&expected),
            "expected structured status/test match {expected:?}; got {matches:?}"
        );
    }
}

#[test]
fn examination_findings_suppress_bare_normal_and_wrong_site_false_positives() {
    let artefact = TerminologyArtefact {
        schema_version: 1,
        terminology_version: "test".to_string(),
        source_release: "test".to_string(),
        refset_id: "test-refset".to_string(),
        generated_at_utc: "test".to_string(),
        concepts: vec![
            concept("277233008", "Anterior rhinorrhea", &["watery discharge"]),
            concept("15188001", "Hearing loss", &["hearing"]),
            concept("246581004", "Peripheral reflex", &["reflex"]),
        ],
        artefact_hash: String::new(),
    };
    let extractor = Extractor::new(artefact).unwrap();

    let response = extractor
        .extract_examination_findings(ExaminationFindingsExtractRequest {
            note_id: Some("exam-8".to_string()),
            objective: "R eye watery discharge. Hearing grossly normal. Red reflex present."
                .to_string(),
            include_suppressed: true,
            refset_id: None,
        })
        .unwrap();

    assert!(!response
        .matches
        .iter()
        .any(|item| item.concept_id == "277233008"));
    assert!(!response
        .matches
        .iter()
        .any(|item| item.concept_id == "15188001"));
    assert!(!response.matches.iter().any(|item| {
        item.concept_id == "246581004" && item.matched_text.eq_ignore_ascii_case("red reflex")
    }));
}

fn concept(concept_id: &str, preferred_term: &str, variants: &[&str]) -> ConceptEntry {
    ConceptEntry {
        concept_id: concept_id.to_string(),
        active: true,
        preferred_term: preferred_term.to_string(),
        descriptions: vec![],
        variants: variants
            .iter()
            .map(|text| TermVariant {
                text: text.to_string(),
                source: "fixture".to_string(),
                description_id: None,
                allow_ambiguous: false,
                requires_numeric_value: false,
            })
            .collect(),
    }
}
