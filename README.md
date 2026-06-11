# SNOMED Finding Extractor

Fast, deterministic, explainable SNOMED CT clinical finding, observable entity, examination finding, and diagnosis extraction for SOAP free text.

This repository is intended to become a standalone component inside a larger EPR. It proposes candidate SNOMED CT finding, observable entity, examination finding, and diagnosis/disorder codes for clinician confirmation; it does not auto-write codes to the clinical record.

## Current Status

This is a v0.2 implementation with:

- Rust extraction core.
- CLI for building terminology artefacts, extracting from SOAP JSON, and auditing dropped ambiguous terms.
- Optional local HTTP API sidecar.
- openEHR value set importer for the supplied symptoms value set manifest.
- Objective-only observable entity extraction from a separate observations value set manifest, with value/unit capture for openEHR quantities.
- Objective-only examination finding extraction from a separate examination findings value set manifest.
- Assessment-only diagnosis/disorder extraction from a separate disorders value set manifest.
- RF2 snapshot/refset importer for future full SNOMED synonym enrichment.
- Runtime UK GP shorthand expansion (`c/o`, `o/e`, `h/o`, `d&v`, `sob`, `fhx`, ...) with span-preserving maps.
- Terminology-derived variants for safe shorthand, including official acronym prefixes, simple diabetes mellitus phrases, morphology variants, flexible body-site sign phrases, examination phrases such as `X on auscultation`, and numeric-only observable labels.
- Deterministic matcher and a clause-scoped assertion engine.
- Synthetic corpus generation and evaluation harness.
- Safety, validation, terminology, and regulatory documentation templates.

## Safety Posture

The engine is deliberately conservative:

- only extracts concepts present in the loaded artefact/refset;
- suppresses negated, uncertain, family-history, non-patient, historical/resolved, conditional, hypothetical, and planned mentions;
- scopes each context cue to the match through bounded, clause-level token analysis, so a cue suppresses only the findings it actually governs (`no fever, has cough` keeps cough; `no cough or wheeze` suppresses both) rather than every concept in the sentence;
- treats the Plan field as review-only by default, with a tightly-scoped completed-action override (`started X for Y` asserts Y);
- returns span-level evidence, the firing rule IDs, and term provenance for every accepted or suppressed match;
- surfaces terms dropped by the ambiguity guard for terminology review instead of dropping them silently;
- logs no raw patient text by default.

## Build A Terminology Artefact

Do not commit TRUD RF2 downloads or generated production artefacts to Git.

From the supplied openEHR value set manifest:

```powershell
cargo run --bin snomed-extract -- build-openehr `
  --valueset "D:\SnoBehr\Refsets for Export\symptoms-20260201.openehr-valueset.json" `
  --output "out\symptoms-20260201.artefact.json"
```

For Objective-field observable entities:

```powershell
cargo run --bin snomed-extract -- build-openehr `
  --valueset "D:\SnoBehr\Refsets for Export\Observables\observations-20260201.openehr-valueset.json" `
  --output "out\observations-20260201.artefact.json"
```

For Objective-field examination findings:

```powershell
cargo run --bin snomed-extract -- build-openehr `
  --valueset "D:\SnoBehr\Refsets for Export\Examination Findings\gp-approved-exam-findings-20260201.openehr-valueset.json" `
  --output "out\examination-findings-20260201.artefact.json"
```

For Assessment-field diagnoses/disorders:

```powershell
cargo run --bin snomed-extract -- build-openehr `
  --valueset "D:\SnoBehr\Refsets for Export\Disorders\diagnoses-20260201.openehr-valueset.json" `
  --output "out\diagnoses-20260201.artefact.json"
```

From RF2 snapshot files:

```powershell
cargo run --bin snomed-extract -- build-rf2 `
  --concept-snapshot "data\rf2\sct2_Concept_Snapshot_INT_*.txt" `
  --description-snapshot "data\rf2\sct2_Description_Snapshot-en_GB_*.txt" `
  --refset-snapshot "data\rf2\der2_Refset_SimpleSnapshot_*.txt" `
  --language-snapshot "data\rf2\der2_cRefset_LanguageSnapshot-en_GB_*.txt" `
  --refset-id "238873041000001104" `
  --terminology-version "http://snomed.info/sct/999000031000000106/version/20260201" `
  --source-release "20260201" `
  --output "out\symptoms-rf2.artefact.json"
```

PowerShell wildcard paths must resolve to concrete files before passing them to the CLI.

## Extract From SOAP JSON

```powershell
cargo run --bin snomed-extract -- extract `
  --artefact "out\symptoms-20260201.artefact.json" `
  --input "fixtures\example-request.json" `
  --include-suppressed
```

Request shape:

```json
{
  "note_id": "example-1",
  "history": "No chest pain. Has cough.",
  "objective": "",
  "assessment": "Chest pain.",
  "plan": "Screen for depression.",
  "include_suppressed": true,
  "refset_id": "238873041000001104"
}
```

## Extract Observable Entities From Objective JSON

```powershell
cargo run --bin snomed-extract -- extract-observables `
  --artefact "out\observations-20260201.artefact.json" `
  --input "fixtures\example-observable-request.json" `
  --include-suppressed
```

Request shape:

```json
{
  "note_id": "example-observation-1",
  "objective": "BP 128/82. HR 76. RR 14. Sats 98%.",
  "include_suppressed": true,
  "refset_id": "785380551000001102"
}
```

## Extract Examination Findings From Objective JSON

```powershell
cargo run --bin snomed-extract -- extract-examination-findings `
  --artefact "out\examination-findings-20260201.artefact.json" `
  --input "fixtures\example-examination-findings-request.json" `
  --include-suppressed
```

Request shape:

```json
{
  "note_id": "example-examination-1",
  "objective": "Chest clear on auscultation, no wheeze.",
  "include_suppressed": true,
  "refset_id": "932266131000001101"
}
```

## Extract Diagnoses From Assessment JSON

```powershell
cargo run --bin snomed-extract -- extract-diagnoses `
  --artefact "out\diagnoses-20260201.artefact.json" `
  --input "fixtures\example-diagnosis-request.json" `
  --include-suppressed
```

Request shape:

```json
{
  "note_id": "example-diagnosis-1",
  "assessment": "Asthma. ?Pneumonia.",
  "include_suppressed": true,
  "refset_id": "782688301000001101"
}
```

## HTTP Sidecar

```powershell
cargo run --features http --bin snomed-serve -- `
  --artefact "out\symptoms-20260201.artefact.json" `
  --observables-artefact "out\observations-20260201.artefact.json" `
  --examination-findings-artefact "out\examination-findings-20260201.artefact.json" `
  --diagnoses-artefact "out\diagnoses-20260201.artefact.json" `
  --host 127.0.0.1 `
  --port 8060
```

Then use:

- `POST /v1/extract` for SOAP finding candidates.
- `POST /v1/extract-observables` for Objective-only observable entity candidates.
- `POST /v1/extract-examination-findings` for Objective-only examination finding candidates.
- `POST /v1/extract-diagnoses` for Assessment-only diagnosis/disorder candidates.

See [API usage](docs/USAGE.md) for integration notes and example API calls. The browser page served at `/` is only a local manual test console.

## Validation

```powershell
cargo run --bin snomed-extract -- generate-synthetic `
  --artefact "out\symptoms-20260201.artefact.json" `
  --output "out\synthetic-cases.json" `
  --max-concepts 500

cargo run --bin snomed-extract -- evaluate `
  --artefact "out\symptoms-20260201.artefact.json" `
  --cases "out\synthetic-cases.json"
```

The acceptance target before production is zero known critical false-positive negation/family/planned-context failures and at least 98% PPV on clinician-reviewed validation data.

## Developer Checks

```powershell
cargo fmt --check
cargo test
cargo clippy --all-targets --features http -- -D warnings
```

## Documentation

- [API contract](docs/API.md)
- [API usage](docs/USAGE.md)
- [Terminology and licensing](docs/TERMINOLOGY.md)
- [Clinical alias sets](docs/ALIASES.md)
- [Validation plan](docs/VALIDATION_PLAN.md)
- [Technical file outline](docs/TECHNICAL_FILE.md)
- [Clinical safety case outline](docs/SAFETY_CASE.md)
- [Hazard log](docs/HAZARD_LOG.md)
- [Architecture decisions](docs/DECISIONS.md)
