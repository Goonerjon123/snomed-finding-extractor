# API Usage

This project is intended to run as a lightweight local API sidecar or embedded Rust library. The browser console is only a manual test harness.

## 1. Build The Terminology Artefact

The API needs a runtime artefact built from the controlled value set export. The current export includes active SNOMED descriptions and synonyms, so a separate alias file is not required for breathlessness terms such as `SOB`, `SOBOE`, `short of breath`, and `breathlessness`.

At build time, the importer also derives a small set of deterministic variants from official descriptions. For example, `PREFIX - expansion` descriptions can contribute the prefix acronym, simple `diabetes mellitus type` descriptions can contribute `Type 2 diabetes`, `Swelling of X` can contribute `swollen X`, and examination descriptions ending `on auscultation` can contribute the shorter base phrase. These variants remain refset-bounded and are blocked if the shorthand becomes ambiguous or loses clinically meaningful specificity. Generated acronym matches also check the original typed casing at runtime: digitless acronyms must be typed as uppercase acronym evidence, which prevents ordinary lowercase words from matching rare acronym syndromes.

At runtime, the matcher also allows bounded morphology over loaded terminology terms: regular plurals and common `-ed`/`-ing` inflections can match the corresponding refset term, for example `target symptoms` to `Target symptom` or `targeted` to `Targeting`. Morphology matches are still refset-bounded, marked with a `:morphology` term source suffix, prefer the longest matching phrase, and are dropped when the morphological form would collapse multiple concepts.

```powershell
$env:RUSTUP_HOME="D:\SNOMED CT EXTRACTOR\.toolchains\rustup"
$env:CARGO_HOME="D:\SNOMED CT EXTRACTOR\.toolchains\cargo"
$env:Path="D:\SNOMED CT EXTRACTOR\.toolchains\cargo\bin;$env:Path"

cargo run --bin snomed-extract -- build-openehr `
  --valueset "D:\SnoBehr\Refsets for Export\symptoms-20260201.openehr-valueset.json" `
  --output "out\symptoms-20260201.artefact.json"
```

Build the Body Site artefact used by the symptom endpoint for post-coordination-like grouping:

```powershell
cargo run --bin snomed-extract -- build-openehr `
  --valueset "D:\SnoBehr\Refsets for Export\Body Site\body-site-20260201.openehr-valueset.json" `
  --output "out\body-site-20260201.artefact.json"
```

Build the Objective-field observable entity artefact from the observations value set:

```powershell
cargo run --bin snomed-extract -- build-openehr `
  --valueset "D:\SnoBehr\Refsets for Export\Observables\observations-20260201.openehr-valueset.json" `
  --output "out\observations-20260201.artefact.json"
```

The observations importer also adds a small versioned set of built-in Objective aliases when the matching observable concept is in the refset, including `BP`, `HR`, `RR`, `SpO2`, `sats`, `O2 sats`, `temp`, and `BMI`. Simple two-word rate observables create numeric-only labels, so `Pulse rate` can match `Pulse 96` or `P: 96`. Official acronym descriptions can also contribute numeric-only labels where the expansion is a simple temperature observable, so `BT - Body temperature` can contribute `T` for `T: 37.8`. Afebrile shorthand such as `afeb 37.2` is also treated as a numeric-only body-temperature label. Those short labels are ignored unless followed by a numeric value.

Build the Objective-field examination findings artefact from the examination findings value set:

```powershell
cargo run --bin snomed-extract -- build-openehr `
  --valueset "D:\SnoBehr\Refsets for Export\Examination Findings\gp-approved-exam-findings-20260201.openehr-valueset.json" `
  --output "out\examination-findings-20260201.artefact.json"
```

The examination findings importer uses the terms and synonyms supplied in the examination findings value set. If the value set contains a description such as `Chest clear on auscultation`, the importer can derive the shorter `Chest clear` phrase from that official description. Body-site sign phrases can also tolerate a small number of intervening modifiers, so terminology such as `Exudate on tonsils` can match text like `Exudate on swollen left tonsil`. Local shorthand with no source description should still be added to the governed value set export or supplied through a reviewed alias file, not hard-coded in the engine.

Build the Assessment-field diagnosis/disorder artefact from the Disorders value set:

```powershell
cargo run --bin snomed-extract -- build-openehr `
  --valueset "D:\SnoBehr\Refsets for Export\Disorders\diagnoses-20260201.openehr-valueset.json" `
  --output "out\diagnoses-20260201.artefact.json"
```

Production deployments should build this artefact during release packaging, record the source value set hash, and keep the generated artefact out of public source control unless SNOMED licensing has been explicitly approved.

## 2. Run The API Sidecar

```powershell
cargo run --features http --bin snomed-serve -- `
  --artefact "out\symptoms-20260201.artefact.json" `
  --body-site-artefact "out\body-site-20260201.artefact.json" `
  --observables-artefact "out\observations-20260201.artefact.json" `
  --examination-findings-artefact "out\examination-findings-20260201.artefact.json" `
  --diagnoses-artefact "out\diagnoses-20260201.artefact.json" `
  --host 127.0.0.1 `
  --port 8060
```

You can run only one endpoint by omitting the other artefact options.
For the symptom endpoint, `--body-site-artefact` is optional; when supplied,
broad symptom matches can carry a nested Body Site SNOMED CT term for openEHR
template population. It requires `--artefact`. The engine does not invent
body-site concepts that are absent from the loaded Body Site artefact.

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

Examination finding extraction endpoint:

```http
POST http://127.0.0.1:8060/v1/extract-examination-findings
Content-Type: application/json
```

Diagnosis/disorder extraction endpoint:

```http
POST http://127.0.0.1:8060/v1/extract-diagnoses
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

## 5. Examination Findings Request Shape

```json
{
  "note_id": "optional-note-id",
  "objective": "Chest clear on auscultation, no wheeze.",
  "include_suppressed": true,
  "refset_id": "932266131000001101"
}
```

The examination findings endpoint deliberately accepts only the `objective` field. It returns SNOMED CT examination finding candidates from the examination findings artefact, not vital-sign observable candidates from the observations artefact.

## 6. Diagnosis Request Shape

```json
{
  "note_id": "optional-note-id",
  "assessment": "Asthma. ?Pneumonia.",
  "include_suppressed": true,
  "refset_id": "782688301000001101"
}
```

The diagnosis endpoint deliberately accepts only the `assessment` field. It returns SNOMED CT diagnosis/disorder candidates from the diagnoses artefact.

## 7. Response Shape

```json
{
  "note_id": "optional-note-id",
  "matches": [
    {
      "concept_id": "418363000",
      "preferred_term": "Itching",
      "field": "history",
      "span_start": 0,
      "span_end": 4,
      "matched_text": "Itch",
      "normalized_match": "itch",
      "term_source": "openehr-description-synonym",
      "body_site": {
        "concept_id": "30021000",
        "preferred_term": "Leg structure",
        "span_start": 7,
        "span_end": 10,
        "matched_text": "leg",
        "normalized_match": "leg",
        "term_source": "openehr-description-synonym"
      },
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
      "explanation": "Suppressed: plan field mentions are review-only unless a completed action asserts them; the mention is the target of a planned action rather than an asserted concept."
    }
  ],
  "terminology_version": "http://snomed.info/sct/999000031000000106/version/20260201",
  "engine_version": "0.1.0",
  "ruleset_version": "ruleset-2026-05-06-v1",
  "artefact_hash": "sha256:...",
  "elapsed_micros": 1268
}
```

For `/v1/extract-observables` and `/v1/extract-examination-findings`, every accepted or suppressed item has `"field": "objective"`. For `/v1/extract-diagnoses`, every accepted or suppressed item has `"field": "assessment"`.

The examination-finding endpoint returns clinically meaningful normal and
non-affirmed exam evidence in `matches`, not only in `suppressed`. For example,
`HS normal` can return Heart sounds with `"assertion": "normal"`, and
`no goitre` can return the Goiter concept with `"assertion": "negated"`,
allowing downstream openEHR exam templates to record normal/negative findings
explicitly. Affirmed matches omit `assertion`; treat that as `affirmed`.

## 8. Example Client Calls

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

Examination findings from the Objective field:

```powershell
$body = @{
  objective = "Chest clear on auscultation, no wheeze."
  include_suppressed = $true
  refset_id = "932266131000001101"
} | ConvertTo-Json

Invoke-RestMethod `
  -Uri "http://127.0.0.1:8060/v1/extract-examination-findings" `
  -Method Post `
  -ContentType "application/json" `
  -Body $body
```

Diagnoses/disorders from the Assessment field:

```powershell
$body = @{
  assessment = "Asthma. ?Pneumonia."
  include_suppressed = $true
  refset_id = "782688301000001101"
} | ConvertTo-Json

Invoke-RestMethod `
  -Uri "http://127.0.0.1:8060/v1/extract-diagnoses" `
  -Method Post `
  -ContentType "application/json" `
  -Body $body
```

## 9. CLI Calls

```powershell
cargo run --bin snomed-extract -- extract-observables `
  --artefact "out\observations-20260201.artefact.json" `
  --input "fixtures\example-observable-request.json"
```

```powershell
cargo run --bin snomed-extract -- extract `
  --artefact "out\symptoms-20260201.artefact.json" `
  --body-site-artefact "out\body-site-20260201.artefact.json" `
  --input "fixtures\example-request.json"
```

```powershell
cargo run --bin snomed-extract -- extract-examination-findings `
  --artefact "out\examination-findings-20260201.artefact.json" `
  --input "fixtures\example-examination-findings-request.json"
```

```powershell
cargo run --bin snomed-extract -- extract-diagnoses `
  --artefact "out\diagnoses-20260201.artefact.json" `
  --input "fixtures\example-diagnosis-request.json"
```

## 10. Integration Rules

- Treat `matches` as candidate concepts for clinician confirmation, not automatically confirmed record entries.
- Use `suppressed` matches to show why terms were not coded, especially negated and planned findings.
- Persist `engine_version`, `ruleset_version`, `terminology_version`, and `artefact_hash` with confirmed decisions for traceability.
- Do not log raw note text in the calling service unless your EPR logging policy explicitly permits it.
- Rebuild and revalidate the artefact whenever the value set, SNOMED release, synonym export, or ruleset changes.

## 11. Browser Console

The server also serves `/` as a local manual test console. This is not the integration surface for the EPR; use `/v1/extract`, `/v1/extract-observables`, `/v1/extract-examination-findings`, and `/v1/extract-diagnoses` for backend integration.
