use snomed_finding_extractor::terminology::{ConceptEntry, TermVariant};
use snomed_finding_extractor::{
    AliasSet, AssertionStatus, ExtractRequest, Extractor, SoapField, TerminologyArtefact,
};
use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("artefacts")
        .join(name)
}

fn alias_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("aliases")
        .join(name)
}

fn extractor() -> Extractor {
    let artefact = TerminologyArtefact::from_path(fixture_path("tiny-symptoms.json")).unwrap();
    Extractor::new(artefact).unwrap()
}

fn extractor_with_aliases() -> Extractor {
    let mut artefact = TerminologyArtefact::from_path(fixture_path("tiny-symptoms.json")).unwrap();
    artefact
        .apply_aliases(AliasSet::from_path(alias_path("tiny-gp-aliases.json")).unwrap())
        .unwrap();
    Extractor::new(artefact).unwrap()
}

fn extractor_with_generic_shared_head_terms() -> Extractor {
    Extractor::new(TerminologyArtefact {
        schema_version: 1,
        terminology_version: "fixture-shared-head".to_string(),
        source_release: "fixture".to_string(),
        refset_id: "fixture-shared-head".to_string(),
        generated_at_utc: "fixture".to_string(),
        concepts: vec![
            ConceptEntry {
                concept_id: "generic-1".to_string(),
                active: true,
                preferred_term: "Alpha marker".to_string(),
                descriptions: vec![],
                variants: vec![TermVariant {
                    text: "alpha marker".to_string(),
                    source: "fixture".to_string(),
                    description_id: None,
                    allow_ambiguous: false,
                    requires_numeric_value: false,
                }],
            },
            ConceptEntry {
                concept_id: "generic-2".to_string(),
                active: true,
                preferred_term: "Beta marker".to_string(),
                descriptions: vec![],
                variants: vec![TermVariant {
                    text: "beta marker".to_string(),
                    source: "fixture".to_string(),
                    description_id: None,
                    allow_ambiguous: false,
                    requires_numeric_value: false,
                }],
            },
        ],
        artefact_hash: String::new(),
    })
    .unwrap()
}

#[test]
fn extracts_affirmed_findings_and_suppresses_unsafe_contexts() {
    let extractor = extractor();
    let response = extractor
        .extract(ExtractRequest {
            note_id: Some("note-1".to_string()),
            history: "No chest pain. Father had diabetes. Has cough but denies asthma.".to_string(),
            assessment: "Chest pain.".to_string(),
            plan: "Screen for depression.".to_string(),
            include_suppressed: true,
            refset_id: Some("fixture-symptoms".to_string()),
            ..ExtractRequest::default()
        })
        .unwrap();

    let positives = response
        .matches
        .iter()
        .map(|item| (item.concept_id.as_str(), item.field))
        .collect::<Vec<_>>();
    let suppressed = response
        .suppressed
        .iter()
        .map(|item| (item.concept_id.as_str(), item.field))
        .collect::<Vec<_>>();

    assert!(positives.contains(&("1000000001", SoapField::Assessment)));
    assert!(positives.contains(&("1000000002", SoapField::History)));
    assert!(!positives.contains(&("1000000001", SoapField::History)));
    assert!(suppressed.contains(&("1000000001", SoapField::History)));
    assert!(suppressed.contains(&("1000000003", SoapField::History)));
    assert!(suppressed.contains(&("1000000004", SoapField::Plan)));
    assert!(suppressed.contains(&("1000000005", SoapField::History)));
}

#[test]
fn accepts_subjective_as_an_alias_for_the_history_field() {
    let request: ExtractRequest =
        serde_json::from_str(r#"{"subjective": "Has cough", "assessment": ""}"#).unwrap();
    assert_eq!(request.history, "Has cough");
}

#[test]
fn reports_terms_dropped_by_the_ambiguity_guard() {
    // Two distinct concepts share the variant "shared term" with no unique
    // exact preferred term, so the guard drops it and the audit records it.
    let artefact = TerminologyArtefact {
        schema_version: 1,
        terminology_version: "test".to_string(),
        source_release: "test".to_string(),
        refset_id: "fixture-symptoms".to_string(),
        generated_at_utc: "test".to_string(),
        artefact_hash: "UNVERIFIED".to_string(),
        concepts: vec![
            concept("1000000001", "Chest pain", &["shared term"]),
            concept("1000000002", "Cough", &["shared term"]),
        ],
    };
    let extractor = Extractor::new(artefact).unwrap();

    let dropped = extractor.dropped_ambiguous_terms();
    assert!(dropped
        .iter()
        .any(|term| term.term == "shared term" && term.competing_concept_count == 2));
}

#[test]
fn suppresses_family_history_after_morphological_plural_matching() {
    let extractor = Extractor::new(TerminologyArtefact {
        schema_version: 1,
        terminology_version: "test".to_string(),
        source_release: "test".to_string(),
        refset_id: "fixture-symptoms".to_string(),
        generated_at_utc: "test".to_string(),
        artefact_hash: "UNVERIFIED".to_string(),
        concepts: vec![concept(
            "1000000099",
            "Target condition",
            &["target condition"],
        )],
    })
    .unwrap();

    let response = extractor
        .extract(ExtractRequest {
            history: "Mum had target conditions.".to_string(),
            include_suppressed: true,
            ..ExtractRequest::default()
        })
        .unwrap();

    assert!(response.matches.is_empty());
    assert_eq!(response.suppressed.len(), 1);
    assert_eq!(response.suppressed[0].concept_id, "1000000099");
    assert_eq!(response.suppressed[0].matched_text, "target conditions");
    assert_eq!(
        response.suppressed[0].assertion,
        AssertionStatus::FamilyHistory
    );
}

#[test]
fn bare_urgency_requires_urinary_context_for_urinary_concept() {
    let extractor = Extractor::new(TerminologyArtefact {
        schema_version: 1,
        terminology_version: "test".to_string(),
        source_release: "test".to_string(),
        refset_id: "fixture-symptoms".to_string(),
        generated_at_utc: "test".to_string(),
        artefact_hash: "UNVERIFIED".to_string(),
        concepts: vec![concept(
            "75088002",
            "Urgent desire to urinate",
            &["urgency"],
        )],
    })
    .unwrap();

    let urinary = extractor
        .extract(ExtractRequest {
            history: "Waterworks trouble. Urgency.".to_string(),
            include_suppressed: true,
            ..ExtractRequest::default()
        })
        .unwrap();
    assert_eq!(urinary.matches.len(), 1);
    assert!(urinary.suppressed.is_empty());

    let bowel = extractor
        .extract(ExtractRequest {
            history: "Loose stools and constipation. Urgency at times.".to_string(),
            include_suppressed: true,
            ..ExtractRequest::default()
        })
        .unwrap();
    assert!(bowel.matches.is_empty());
    assert_eq!(bowel.suppressed.len(), 1);
    assert_eq!(bowel.suppressed[0].assertion, AssertionStatus::Ambiguous);
}

#[test]
fn bare_smell_descriptors_require_urinary_context_for_malodorous_urine() {
    let extractor = Extractor::new(TerminologyArtefact {
        schema_version: 1,
        terminology_version: "test".to_string(),
        source_release: "test".to_string(),
        refset_id: "fixture-symptoms".to_string(),
        generated_at_utc: "test".to_string(),
        artefact_hash: "UNVERIFIED".to_string(),
        concepts: vec![concept(
            "278017001",
            "Malodorous urine",
            &["strong smelling", "smelly"],
        )],
    })
    .unwrap();

    let urinary = extractor
        .extract(ExtractRequest {
            history: "Urine cloudy and strong-smelling yesterday.".to_string(),
            include_suppressed: true,
            ..ExtractRequest::default()
        })
        .unwrap();
    assert_eq!(urinary.matches.len(), 1);
    assert!(urinary.suppressed.is_empty());

    let non_urinary = extractor
        .extract(ExtractRequest {
            history: "The discharge is strong-smelling.".to_string(),
            include_suppressed: true,
            ..ExtractRequest::default()
        })
        .unwrap();
    assert!(non_urinary.matches.is_empty());
    assert_eq!(non_urinary.suppressed.len(), 1);
    assert_eq!(
        non_urinary.suppressed[0].assertion,
        AssertionStatus::Ambiguous
    );
}

#[test]
fn anatomical_shorthand_pain_prefers_specific_site_concept() {
    let extractor = Extractor::new(TerminologyArtefact {
        schema_version: 1,
        terminology_version: "test".to_string(),
        source_release: "test".to_string(),
        refset_id: "fixture-symptoms".to_string(),
        generated_at_utc: "test".to_string(),
        artefact_hash: "UNVERIFIED".to_string(),
        concepts: vec![
            concept("22253000", "Pain", &["pain"]),
            concept(
                "285388000",
                "Right upper quadrant pain",
                &["right upper quadrant pain"],
            ),
        ],
    })
    .unwrap();

    let response = extractor
        .extract(ExtractRequest {
            history: "Recurrent severe RUQ pain after fatty meals.".to_string(),
            include_suppressed: true,
            ..ExtractRequest::default()
        })
        .unwrap();

    assert_eq!(response.matches.len(), 1);
    assert_eq!(response.matches[0].concept_id, "285388000");
    assert_eq!(
        response.matches[0].preferred_term,
        "Right upper quadrant pain"
    );
    assert_eq!(response.matches[0].matched_text, "RUQ pain");
    assert!(response.suppressed.is_empty());
}

#[test]
fn head_first_site_mentions_prefer_specific_site_concepts() {
    let extractor = Extractor::new(TerminologyArtefact {
        schema_version: 1,
        terminology_version: "test".to_string(),
        source_release: "test".to_string(),
        refset_id: "fixture-symptoms".to_string(),
        generated_at_utc: "test".to_string(),
        artefact_hash: "UNVERIFIED".to_string(),
        concepts: vec![
            concept("22253000", "Pain", &["pain"]),
            concept("300848003", "Mass of body structure", &["lump"]),
            concept("1000000301", "Pain in calf", &["calf pain"]),
            concept("1000000302", "Breast lump", &["breast lump"]),
        ],
    })
    .unwrap();

    let calf = extractor
        .extract(ExtractRequest {
            history: "Cramping pain both calves on walking.".to_string(),
            include_suppressed: true,
            ..ExtractRequest::default()
        })
        .unwrap();
    assert_eq!(calf.matches.len(), 1);
    assert_eq!(calf.matches[0].preferred_term, "Pain in calf");
    assert_eq!(calf.matches[0].matched_text, "pain both calves");

    let breast = extractor
        .extract(ExtractRequest {
            history: "Lump R breast, painless.".to_string(),
            include_suppressed: true,
            ..ExtractRequest::default()
        })
        .unwrap();
    assert_eq!(breast.matches.len(), 1);
    assert_eq!(breast.matches[0].preferred_term, "Breast lump");
    assert_eq!(breast.matches[0].matched_text, "Lump R breast");
}

#[test]
fn pv_bleeding_and_lower_abdominal_cramping_extract_specific_concepts() {
    let extractor = Extractor::new(TerminologyArtefact {
        schema_version: 1,
        terminology_version: "test".to_string(),
        source_release: "test".to_string(),
        refset_id: "fixture-symptoms".to_string(),
        generated_at_utc: "test".to_string(),
        artefact_hash: "UNVERIFIED".to_string(),
        concepts: vec![
            concept("22253000", "Pain", &["pain"]),
            concept("289530006", "Vaginal bleeding", &["vaginal bleeding"]),
            concept(
                "54586004",
                "Lower abdominal pain",
                &["lower abdominal cramping"],
            ),
        ],
    })
    .unwrap();

    let response = extractor
        .extract(ExtractRequest {
            history: "Light PV bleeding. Mild lower abdo cramping. No shoulder-tip pain."
                .to_string(),
            include_suppressed: true,
            ..ExtractRequest::default()
        })
        .unwrap();

    let positives = response
        .matches
        .iter()
        .map(|item| (item.concept_id.as_str(), item.matched_text.as_str()))
        .collect::<Vec<_>>();
    assert!(positives.contains(&("289530006", "PV bleeding")));
    assert!(positives.contains(&("54586004", "lower abdo cramping")));
    assert!(!positives
        .iter()
        .any(|(concept_id, _)| *concept_id == "22253000"));
    assert_eq!(response.suppressed.len(), 1);
    assert_eq!(response.suppressed[0].preferred_term, "Pain");
    assert_eq!(response.suppressed[0].matched_text, "pain");
    assert_eq!(response.suppressed[0].assertion, AssertionStatus::Negated);
}

fn concept(
    concept_id: &str,
    preferred_term: &str,
    variants: &[&str],
) -> snomed_finding_extractor::terminology::ConceptEntry {
    snomed_finding_extractor::terminology::ConceptEntry {
        concept_id: concept_id.to_string(),
        active: true,
        preferred_term: preferred_term.to_string(),
        descriptions: vec![],
        variants: variants
            .iter()
            .map(|text| snomed_finding_extractor::terminology::TermVariant {
                text: text.to_string(),
                source: "fixture".to_string(),
                description_id: None,
                allow_ambiguous: false,
                requires_numeric_value: false,
            })
            .collect(),
    }
}

#[test]
fn omits_suppressed_matches_unless_requested() {
    let extractor = extractor();
    let response = extractor
        .extract(ExtractRequest {
            history: "No chest pain.".to_string(),
            include_suppressed: false,
            ..ExtractRequest::default()
        })
        .unwrap();

    assert!(response.matches.is_empty());
    assert!(response.suppressed.is_empty());
}

#[test]
fn extracts_gp_breathlessness_aliases() {
    let extractor = extractor_with_aliases();
    let response = extractor
        .extract(ExtractRequest {
            history: "Feels short of breath. SOBOE after stairs. No SOB at rest.".to_string(),
            include_suppressed: true,
            ..ExtractRequest::default()
        })
        .unwrap();

    let positives = response
        .matches
        .iter()
        .map(|item| (item.concept_id.as_str(), item.matched_text.as_str()))
        .collect::<Vec<_>>();
    let suppressed = response
        .suppressed
        .iter()
        .map(|item| (item.concept_id.as_str(), item.matched_text.as_str()))
        .collect::<Vec<_>>();

    assert!(positives.contains(&("1000000006", "short of breath")));
    assert!(positives.contains(&("1000000007", "SOBOE")));
    assert!(suppressed.contains(&("1000000006", "SOB")));
}

#[test]
fn suppresses_coordinated_shared_head_findings_under_negation() {
    let extractor = extractor_with_generic_shared_head_terms();
    let response = extractor
        .extract(ExtractRequest {
            history: "No alpha/beta marker. Alpha marker later present.".to_string(),
            include_suppressed: true,
            refset_id: Some("fixture-shared-head".to_string()),
            ..ExtractRequest::default()
        })
        .unwrap();

    let positives = response
        .matches
        .iter()
        .map(|item| (item.concept_id.as_str(), item.matched_text.as_str()))
        .collect::<Vec<_>>();
    let suppressed = response
        .suppressed
        .iter()
        .map(|item| {
            (
                item.concept_id.as_str(),
                item.matched_text.as_str(),
                item.assertion,
            )
        })
        .collect::<Vec<_>>();

    assert!(positives.contains(&("generic-1", "Alpha marker")));
    assert!(!positives.iter().any(|item| matches!(item.0, "generic-2")));
    assert!(suppressed.contains(&("generic-1", "alpha/beta marker", AssertionStatus::Negated)));
    assert!(suppressed.contains(&("generic-2", "beta marker", AssertionStatus::Negated)));
}
