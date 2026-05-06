use snomed_finding_extractor::rf2::build_from_openehr_valueset;
use snomed_finding_extractor::{ExtractRequest, Extractor};
use std::path::PathBuf;

#[test]
fn builds_artefact_from_openehr_valueset_manifest() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("synthetic-valueset.openehr-valueset.json");
    let artefact = build_from_openehr_valueset(path).unwrap();

    assert_eq!(artefact.refset_id, "238873041000001104");
    assert_eq!(artefact.source_release, "20260201");
    assert_eq!(artefact.concepts.len(), 4);
    assert!(artefact.artefact_hash.starts_with("sha256:"));

    let extractor = Extractor::new(artefact).unwrap();
    let response = extractor
        .extract(ExtractRequest {
            assessment: "Chest pain and cough.".to_string(),
            ..ExtractRequest::default()
        })
        .unwrap();

    assert_eq!(response.matches.len(), 2);
}

#[test]
fn imports_synonyms_and_derives_safe_gp_acronym_variants() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("synthetic-valueset.openehr-valueset.json");
    let artefact = build_from_openehr_valueset(path).unwrap();
    let extractor = Extractor::new(artefact).unwrap();
    let response = extractor
        .extract(ExtractRequest {
            history: "Breathlessness. Feels short of breath. SOBOE. No SOB at rest.".to_string(),
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

    assert!(positives.contains(&("1000000006", "Breathlessness")));
    assert!(positives.contains(&("1000000006", "short of breath")));
    assert!(positives.contains(&("1000000007", "SOBOE")));
    assert!(suppressed.contains(&("1000000006", "SOB")));
}
