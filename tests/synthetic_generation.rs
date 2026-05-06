use snomed_finding_extractor::synthetic::generate_synthetic_cases;
use snomed_finding_extractor::{Extractor, TerminologyArtefact};
use std::path::PathBuf;

#[test]
fn generated_synthetic_cases_round_trip_through_extractor() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("artefacts")
        .join("tiny-symptoms.json");
    let artefact = TerminologyArtefact::from_path(path).unwrap();
    let cases = generate_synthetic_cases(&artefact, 1);
    let extractor = Extractor::new(artefact).unwrap();

    assert_eq!(cases.len(), 7);

    for case in cases {
        let response = extractor.extract(case.request.clone()).unwrap();
        for expected in case.expected_positive_concept_ids {
            assert!(
                response
                    .matches
                    .iter()
                    .any(|item| item.concept_id == expected),
                "expected positive concept in case {}",
                case.id
            );
        }
        for expected in case.expected_suppressed_concept_ids {
            assert!(
                response
                    .suppressed
                    .iter()
                    .any(|item| item.concept_id == expected),
                "expected suppressed concept in case {}",
                case.id
            );
        }
    }
}
