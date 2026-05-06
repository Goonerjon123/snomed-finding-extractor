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
