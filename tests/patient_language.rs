use serde_json::json;
use snomed_finding_extractor::rf2::{
    build_from_openehr_valueset, build_from_rf2_snapshot, Rf2BuildInput,
};
use snomed_finding_extractor::{AssertionStatus, ExtractRequest, Extractor, TerminologyArtefact};
use std::fs;

fn build(members: serde_json::Value) -> TerminologyArtefact {
    try_build(members).unwrap()
}

fn try_build(members: serde_json::Value) -> snomed_finding_extractor::Result<TerminologyArtefact> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("symptoms.json");
    fs::write(
        &path,
        serde_json::to_vec(&json!({
            "id": "patient-language-test",
            "terminology": { "releaseDate": "test" },
            "valueSet": {},
            "members": members
        }))
        .unwrap(),
    )
    .unwrap();
    build_from_openehr_valueset(path)
}

fn symptoms() -> TerminologyArtefact {
    build(json!([
        {"code":"29857009","display":"Chest pain","active":true,"system":"http://snomed.info/sct"},
        {"code":"21522001","display":"Abdominal pain","active":true,"system":"http://snomed.info/sct"},
        {"code":"161891005","display":"Backache","active":true,"system":"http://snomed.info/sct"},
        {"code":"25064002","display":"Headache","active":true,"system":"http://snomed.info/sct"},
        {"code":"422587007","display":"Nausea","active":true,"system":"http://snomed.info/sct"},
        {"code":"422400008","display":"Vomiting","active":true,"system":"http://snomed.info/sct"},
        {"code":"49650001","display":"Dysuria","active":true,"system":"http://snomed.info/sct"},
        {"code":"267036007","display":"Dyspnea","active":true,"system":"http://snomed.info/sct"}
    ]))
}

#[test]
fn patient_wording_has_specific_codes_original_spans_and_alias_provenance() {
    let extractor = Extractor::new(symptoms()).unwrap();
    for (phrase, code) in [
        ("Sore chest", "29857009"),
        ("chest feels sore", "29857009"),
        ("chest hurts", "29857009"),
        ("aching chest", "29857009"),
        ("sore tummy", "21522001"),
        ("belly hurts", "21522001"),
        ("sore back", "161891005"),
        ("head hurts", "25064002"),
        ("feeling sick", "422587007"),
        ("throwing up", "422400008"),
        ("hurts to pee", "49650001"),
        ("out of breath", "267036007"),
    ] {
        let text = format!("Patient says: ‘{phrase}’ today.");
        let response = extractor
            .extract(ExtractRequest {
                history: text.clone(),
                include_suppressed: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(response.matches.len(), 1, "{text}: {response:?}");
        let found = &response.matches[0];
        assert_eq!(found.concept_id, code, "{text}");
        assert_eq!(found.matched_text, phrase);
        assert_eq!(&text[found.span_start..found.span_end], phrase);
        assert_eq!(found.term_source, "clinical_alias:patient-language-v1");
        assert!(!found.rule_ids.is_empty());
        assert!(response.suppressed.is_empty(), "{text}: {response:?}");
    }
}

#[test]
fn patient_phrases_keep_assertion_controls() {
    let extractor = Extractor::new(symptoms()).unwrap();
    for phrase in [
        "sore chest",
        "sore tummy",
        "sore back",
        "head hurts",
        "feeling sick",
        "throwing up",
        "hurts to pee",
        "out of breath",
    ] {
        for (text, assertion) in [
            (format!("No {phrase}."), AssertionStatus::Negated),
            (format!("Denies {phrase}."), AssertionStatus::Negated),
            (format!("Possible {phrase}."), AssertionStatus::Uncertain),
            (
                format!("Father has {phrase}."),
                AssertionStatus::FamilyHistory,
            ),
            (
                format!("History of {phrase}."),
                AssertionStatus::HistoricalOrResolved,
            ),
            (
                format!("If {phrase}, seek review."),
                AssertionStatus::Conditional,
            ),
        ] {
            let response = extractor
                .extract(ExtractRequest {
                    history: text.clone(),
                    include_suppressed: true,
                    ..Default::default()
                })
                .unwrap();
            assert!(response.matches.is_empty(), "{text}: {response:?}");
            assert_eq!(response.suppressed.len(), 1, "{text}: {response:?}");
            assert_eq!(response.suppressed[0].assertion, assertion, "{text}");
            assert_eq!(response.suppressed[0].matched_text, phrase);
        }
        let response = extractor
            .extract(ExtractRequest {
                plan: format!("Review {phrase}."),
                include_suppressed: true,
                ..Default::default()
            })
            .unwrap();
        assert!(response.matches.is_empty(), "{phrase}: {response:?}");
        assert_eq!(response.suppressed.len(), 1, "{phrase}: {response:?}");
    }
}

#[test]
fn ambiguous_or_unrelated_language_does_not_invent_a_symptom() {
    let extractor = Extractor::new(symptoms()).unwrap();
    for text in [
        "sore",
        "sick",
        "poorly",
        "chest",
        "sore throat",
        "sore spot on chest",
        "chest of drawers",
        "wore chest",
        "heart attack",
        "sick leave",
        "feeling sick of work",
        "feels sick of taking tablets",
        "been sick",
    ] {
        let response = extractor
            .extract(ExtractRequest {
                history: text.into(),
                include_suppressed: true,
                ..Default::default()
            })
            .unwrap();
        assert!(response.matches.is_empty(), "{text}: {response:?}");
    }
}

#[test]
fn terminology_target_mismatch_fails_the_build() {
    let result = try_build(json!([
        {"code":"29857009","display":"Unexpected meaning","active":true,"system":"http://snomed.info/sct"}
    ]));
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("expected concept 29857009 preferred term Chest pain"));
}

#[test]
fn clause_boundaries_keep_separate_patient_symptoms() {
    let extractor = Extractor::new(symptoms()).unwrap();
    for (text, accepted_code) in [
        ("No sore chest, but throwing up.", "422400008"),
        ("Father has sore chest. Patient has sore tummy.", "21522001"),
        ("Feeling sick. Of note, appetite is reduced.", "422587007"),
    ] {
        let response = extractor
            .extract(ExtractRequest {
                history: text.into(),
                include_suppressed: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(response.matches.len(), 1, "{text}: {response:?}");
        assert_eq!(response.matches[0].concept_id, accepted_code, "{text}");
    }
}

/// Opt-in integration evidence using a locally supplied, licensed value set.
/// No production terminology is checked into this repository.
#[test]
#[ignore = "Set SNOMED_SYMPTOMS_VALUESET to a licensed symptoms manifest"]
fn packaged_terminology_preserves_patient_phrase_codes_and_assertions() {
    let path = std::env::var("SNOMED_SYMPTOMS_VALUESET").unwrap();
    let artefact = build_from_openehr_valueset(path).unwrap();
    let extractor = Extractor::new(artefact).unwrap();
    let aliases: snomed_finding_extractor::AliasSet =
        serde_json::from_str(include_str!("../aliases/patient-language-v1.json")).unwrap();
    for alias in aliases.aliases {
        for phrase in alias.terms {
            for (prefix, expected) in [
                ("", None),
                ("No ", Some(AssertionStatus::Negated)),
                ("Possible ", Some(AssertionStatus::Uncertain)),
                ("Father has ", Some(AssertionStatus::FamilyHistory)),
                ("History of ", Some(AssertionStatus::HistoricalOrResolved)),
                ("If ", Some(AssertionStatus::Conditional)),
            ] {
                let text = format!("{prefix}{phrase}.");
                let response = extractor
                    .extract(ExtractRequest {
                        history: text.clone(),
                        include_suppressed: true,
                        ..Default::default()
                    })
                    .unwrap();
                if let Some(assertion) = expected {
                    assert!(response.matches.is_empty(), "{text}: {response:?}");
                    assert_eq!(response.suppressed.len(), 1, "{text}: {response:?}");
                    assert_eq!(
                        response.suppressed[0].concept_id, alias.concept_id,
                        "{text}"
                    );
                    assert_eq!(response.suppressed[0].assertion, assertion, "{text}");
                } else {
                    assert_eq!(response.matches.len(), 1, "{text}: {response:?}");
                    assert_eq!(response.matches[0].concept_id, alias.concept_id, "{text}");
                    assert_eq!(response.matches[0].matched_text, phrase, "{text}");
                    assert!(response.suppressed.is_empty(), "{text}: {response:?}");
                }
            }
        }
    }
}

#[test]
fn aliases_do_not_add_absent_or_inactive_concepts() {
    let artefact = build(json!([
        {"code":"29857009","display":"Chest pain","active":false,"system":"http://snomed.info/sct"},
        {"code":"test-cough","display":"Cough","active":true,"system":"http://snomed.info/sct"}
    ]));
    assert_eq!(artefact.concepts.len(), 1);
    let response = Extractor::new(artefact)
        .unwrap()
        .extract(ExtractRequest {
            history: "Sore chest, sore tummy, throwing up.".into(),
            ..Default::default()
        })
        .unwrap();
    assert!(response.matches.is_empty());
}

#[test]
fn official_synonym_wins_over_patient_language_alias() {
    let artefact = build(json!([
        {"code":"29857009","display":"Chest pain","active":true,"system":"http://snomed.info/sct"},
        {"code":"test-official","display":"Official fixture finding","active":true,"system":"http://snomed.info/sct",
         "descriptions":[{"descriptionId":"test-description","term":"Sore chest","type":"synonym","active":true}]}
    ]));
    let response = Extractor::new(artefact)
        .unwrap()
        .extract(ExtractRequest {
            history: "Sore chest".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(response.matches.len(), 1);
    assert_eq!(response.matches[0].concept_id, "test-official");
    assert_eq!(
        response.matches[0].term_source,
        "openehr-description-synonym"
    );
}

#[test]
fn rf2_import_also_builds_patient_language_aliases() {
    let dir = tempfile::tempdir().unwrap();
    let concepts = dir.path().join("concepts.tsv");
    let descriptions = dir.path().join("descriptions.tsv");
    let refset = dir.path().join("refset.tsv");
    fs::write(&concepts, "id\tactive\n29857009\t1\n").unwrap();
    fs::write(&descriptions, "id\tactive\tconceptId\tterm\ttypeId\nfixture-description\t1\t29857009\tChest pain\t900000000000013009\n").unwrap();
    fs::write(
        &refset,
        "active\trefsetId\treferencedComponentId\n1\tfixture\t29857009\n",
    )
    .unwrap();
    let artefact = build_from_rf2_snapshot(Rf2BuildInput {
        concept_snapshot: concepts.clone(),
        description_snapshot: descriptions.clone(),
        refset_snapshot: refset.clone(),
        language_snapshot: None,
        refset_id: "fixture".into(),
        terminology_version: "test".into(),
        source_release: "test".into(),
    })
    .unwrap();
    let response = Extractor::new(artefact)
        .unwrap()
        .extract(ExtractRequest {
            history: "Sore chest".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(response.matches.len(), 1);
    assert_eq!(response.matches[0].concept_id, "29857009");

    // An official synonym on another in-scope concept must take precedence.
    fs::write(&concepts, "id\tactive\n29857009\t1\ntest-official\t1\n").unwrap();
    fs::write(&descriptions, "id\tactive\tconceptId\tterm\ttypeId\nfixture-description\t1\t29857009\tChest pain\t900000000000013009\nfixture-other\t1\ttest-official\tOfficial fixture finding\t900000000000013009\nfixture-synonym\t1\ttest-official\tSore chest\t900000000000013009\n").unwrap();
    fs::write(&refset, "active\trefsetId\treferencedComponentId\n1\tfixture\t29857009\n1\tfixture\ttest-official\n").unwrap();
    let artefact = build_from_rf2_snapshot(Rf2BuildInput {
        concept_snapshot: concepts,
        description_snapshot: descriptions,
        refset_snapshot: refset,
        language_snapshot: None,
        refset_id: "fixture".into(),
        terminology_version: "test".into(),
        source_release: "test".into(),
    })
    .unwrap();
    let response = Extractor::new(artefact)
        .unwrap()
        .extract(ExtractRequest {
            history: "Sore chest".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(response.matches.len(), 1);
    assert_eq!(response.matches[0].concept_id, "test-official");
    assert_eq!(response.matches[0].term_source, "rf2-synonym");
}

#[test]
fn patient_aliases_survive_artefact_reload_and_are_covered_by_hash() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("artefact.json");
    let mut artefact = symptoms();
    artefact.write_pretty_json(&path).unwrap();
    let loaded = TerminologyArtefact::from_path(&path).unwrap();
    assert_eq!(artefact.artefact_hash, loaded.artefact_hash);
    let response = Extractor::new(loaded)
        .unwrap()
        .extract(ExtractRequest {
            history: "Sore chest".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(response.matches[0].concept_id, "29857009");
    let mut tampered: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    tampered["concepts"][0]["variants"]
        .as_array_mut()
        .unwrap()
        .retain(|v| v["source"] != "clinical_alias:patient-language-v1");
    fs::write(&path, serde_json::to_vec(&tampered).unwrap()).unwrap();
    assert!(TerminologyArtefact::from_path(&path).is_err());
}
