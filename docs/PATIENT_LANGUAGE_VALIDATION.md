# Patient-language validation — 2026-11-03

Engineering evidence for the initial alias set; clinical acceptance and CSO
review remain outstanding. See [H-016](HAZARD_LOG.md#h-016--patient-language-maps-to-an-incorrect-clinical-concept).

## Reproduction

Baseline: upstream `7baa03d3251dbf4725e534358bb1bed26ac5f00e`, engine 0.2.0,
ruleset `ruleset-2026-06-15-v6`. Building the packaged symptoms manifest and
extracting `{"history":"Sore chest","include_suppressed":true}` returned no
accepted or suppressed candidates. The manifest contains active `29857009`
`Chest pain`, but does not contain `sore chest` as a description or alias. This
does not establish whether that phrase exists in another SNOMED release or
extension.

With the rebuilt artefact, the same request returned one affirmed candidate:
`29857009`, `Chest pain`, original span `[0, 10)`, matched text `Sore chest`,
source `clinical_alias:patient-language-v1`.

## Inputs and checks

- Engine: 0.2.0; ruleset: `ruleset-2026-11-03-v7`.
- UK terminology: `http://snomed.info/sct/999000031000000106/version/20260201`.
- Symptoms refset: `238873041000001104`.
- Uncompressed packaged manifest SHA-256: `25db8c0231891cbc64332b1920e6e9070453d2decd7dd97d1aa2bfce26551e9d`.
- Alias file SHA-256: `7f1a7d2066c182a0f8f23f23238e8c26deab6bb25b5eb21524c662da6dd51bc2`.
- Local test environment: macOS arm64, Rust 1.99.1.
- `cargo fmt --check`: passed. Includes a formatting-only adjustment in the existing Plan code required by rustfmt.
- `cargo clippy --locked --all-targets --features http -- -D warnings`: passed.
- `SNOMED_SYMPTOMS_VALUESET=... cargo test --locked --features http -- --include-ignored`: 171 tests passed.
- The opt-in packaged-terminology test covers all 50 phrases in six assertion contexts: 300 cases. Assertions include exact target code, original matched text for positive cases, no positive matches for suppressed cases, and expected suppression status.
- Synthetic fixtures also exercise Plan suppression, clause boundaries, ambiguous prose, absent/inactive targets, mismatched preferred terms, official synonym collisions in both importers, Unicode source spans, and hash-verified artefact reload/tampering.
- Local HTTP `POST /v1/extract`: six smoke cases passed (`Sore chest`, `No sore chest`, `Father has sore chest`, `Possible sore chest`, `feeling sick of work`, `hurts to pee`). The temporary sidecar was stopped afterwards.
- `git diff --check`: passed.

## Limits and integration

This is local/synthetic evidence using a packaged terminology source, not an
authenticated Ensemble acceptance run, a clinician-reviewed validation set,
a performance qualification, or evidence that the production deployment has
changed. The 98% clinical PPV release target has not been measured here.

No consumer code, deployment pin or running hosted service was changed.
Ensemble's `clinical-list-answers.patch` currently expects the old
`RULESET_VERSION` line and fails its patch check against this revision. Refresh
that hunk when adopting this release, retain the consumer's existing assertion
patches, rebuild/reload all artefacts, and run combined integration tests before
deployment. `plan-context-boundaries.patch` passed a standalone apply check.

Further patient wording should enter the same versioned alias and regression
process. The 50 phrases are an initial bounded set, not comprehensive coverage.
