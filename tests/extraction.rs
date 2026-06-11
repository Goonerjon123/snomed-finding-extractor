use snomed_finding_extractor::{
    AliasSet, ExtractRequest, Extractor, SoapField, TerminologyArtefact,
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
