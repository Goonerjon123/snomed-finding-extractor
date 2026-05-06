# API Usage

This project is intended to run as a lightweight local API sidecar or embedded Rust library. The browser console is only a manual test harness.

## 1. Build The Terminology Artefact

The API needs a runtime artefact built from the controlled value set export. The current export includes active SNOMED descriptions and synonyms, so a separate alias file is not required for breathlessness terms such as `SOB`, `SOBOE`, `short of breath`, and `breathlessness`.

```powershell
$env:RUSTUP_HOME="D:\SNOMED CT EXTRACTOR\.toolchains\rustup"
$env:CARGO_HOME="D:\SNOMED CT EXTRACTOR\.toolchains\cargo"
$env:Path="D:\SNOMED CT EXTRACTOR\.toolchains\cargo\bin;$env:Path"

cargo run --bin snomed-extract -- build-openehr `
  --valueset "D:\SnoBehr\Refsets for Export\symptoms-20260201.openehr-valueset.json" `
  --output "out\symptoms-20260201.artefact.json"
```

Build the Objective-field observable entity artefact from the observations value set:

```powershell
cargo run --bin snomed-extract -- build-openehr `
  --valueset "D:\SnoBehr\Refsets for Export\Observables\observations-20260201.openehr-valueset.json" `
  --output "out\observations-20260201.artefact.json"
```

The observations importer also adds a small versioned set of built-in Objective aliases when the matching observable concept is in the refset, including `BP`, `HR`, `RR`, `SpO2`, `sats`, `O2 sats`, `temp`, and `BMI`.

Production deployments should build this artefact during release packaging, record the source value set hash, and keep the generated artefact out of public source control unless SNOMED licensing has been explicitly approved.

## 2. Run The API Sidecar

```powershell
cargo run --features http --bin snomed-serve -- `
  --artefact "out\symptoms-20260201.artefact.json" `
  --observables-artefact "out\observations-20260201.artefact.json" `
  --host 127.0.0.1 `
  --port 8060
```

You can run only the observable endpoint by omitting `--artefact` and supplying `--observables-artefact`.

Health check:

```http
GET http://127.0.0.1:8060/healthz
```

Finding extraction endpoint:

```http
POST http://127.0.0.1:8060/v1/extract
Content-Type: application/json
```

Observable entity extraction endpoint:

```http
POST http://127.0.0.1:8060/v1/extract-observables
Content-Type: application/json
```

## 3. Request Shape

```json
{
  "note_id": "optional-note-id",
  "history": "No chest pain. Feels short of breath. SOBOE after stairs.",
  "objective": "",
  "assessment": "Breathlessness worsening.",
  "plan": "Screen for shortness of breath on exertion.",
  "include_suppressed": true,
  "refset_id": "238873041000001104"
}
```

Fields:

- `history`, `objective`, `assessment`, `plan`: SOAP free text fields.
- `include_suppressed`: when true, returns negated/planned/family-history/uncertain matches for audit and UI explanation.
- `refset_id`: optional guard. If provided, it must match the loaded artefact refset.
- `note_id`: optional caller-owned identifier echoed in the response.

## 4. Observable Request Shape

```json
{
  "note_id": "optional-note-id",
  "objective": "BP 128/82. HR 76. RR 14. Sats 98%. No temp recorded.",
  "include_suppressed": true,
  "refset_id": "785380551000001102"
}
```

The observable endpoint deliberately accepts only the `objective` field. It returns SNOMED CT observable entity candidates from the observations artefact, not finding candidates from the symptoms artefact.

## 5. Response Shape

```json
{
  "note_id": "optional-note-id",
  "matches": [
    {
      "concept_id": "267036007",
      "preferred_term": "Dyspnea",
      "field": "history",
      "span_start": 22,
      "span_end": 37,
      "matched_text": "short of breath",
      "normalized_match": "short of breath",
      "confidence": 0.92,
      "rule_ids": ["ASSERT_AFFIRMED_PATIENT_FINDING"],
      "explanation": "Accepted as an affirmed patient finding in the history field; no suppression rule fired."
    }
  ],
  "suppressed": [
    {
      "concept_id": "60845006",
      "preferred_term": "Dyspnea on exertion",
      "field": "plan",
      "span_start": 11,
      "span_end": 42,
      "matched_text": "shortness of breath on exertion",
      "normalized_match": "shortness of breath on exertion",
      "assertion": "planned",
      "rule_ids": ["PLAN_FIELD_REVIEW_ONLY", "CTX_PLANNED_ACTION"],
      "explanation": "Suppressed: plan field mentions are review-only unless a future ruleset explicitly permits them; the mention is part of a planned action rather than an asserted concept."
    }
  ],
  "terminology_version": "http://snomed.info/sct/999000031000000106/version/20260201",
  "engine_version": "0.1.0",
  "ruleset_version": "ruleset-2026-05-06-v1",
  "artefact_hash": "sha256:...",
  "elapsed_micros": 1268
}
```

For `/v1/extract-observables`, every accepted or suppressed item has `"field": "objective"`.

## 6. Example Client Calls

```powershell
$body = @{
  history = "Feels short of breath. SOBOE after stairs. No SOB at rest."
  objective = ""
  assessment = "Breathlessness worsening."
  plan = "Screen for shortness of breath on exertion."
  include_suppressed = $true
  refset_id = "238873041000001104"
} | ConvertTo-Json

Invoke-RestMethod `
  -Uri "http://127.0.0.1:8060/v1/extract" `
  -Method Post `
  -ContentType "application/json" `
  -Body $body
```

Observable entities from the Objective field:

```powershell
$body = @{
  objective = "BP 128/82. HR 76. RR 14. Sats 98%. No temp recorded."
  include_suppressed = $true
  refset_id = "785380551000001102"
} | ConvertTo-Json

Invoke-RestMethod `
  -Uri "http://127.0.0.1:8060/v1/extract-observables" `
  -Method Post `
  -ContentType "application/json" `
  -Body $body
```

## 7. CLI Calls

```powershell
cargo run --bin snomed-extract -- extract-observables `
  --artefact "out\observations-20260201.artefact.json" `
  --input "fixtures\example-observable-request.json"
```

## 8. Integration Rules

- Treat `matches` as candidate concepts for clinician confirmation, not automatically confirmed record entries.
- Use `suppressed` matches to show why terms were not coded, especially negated and planned findings.
- Persist `engine_version`, `ruleset_version`, `terminology_version`, and `artefact_hash` with confirmed decisions for traceability.
- Do not log raw note text in the calling service unless your EPR logging policy explicitly permits it.
- Rebuild and revalidate the artefact whenever the value set, SNOMED release, synonym export, or ruleset changes.

## 9. Browser Console

The server also serves `/` as a local manual test console. This is not the integration surface for the EPR; use `/v1/extract` and `/v1/extract-observables` for backend integration.
