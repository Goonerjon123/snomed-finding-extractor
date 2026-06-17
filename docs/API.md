# API Contract

## Intended Use

The engine supports clinicians by identifying candidate SNOMED CT finding codes mentioned in SOAP free text, candidate SNOMED CT observable entity codes mentioned in the Objective field, candidate SNOMED CT examination finding codes mentioned in the Objective field, and candidate SNOMED CT diagnosis/disorder codes mentioned in the Assessment field for clinician review and confirmation.

## Finding Request

`POST /v1/extract`

```json
{
  "note_id": "optional-local-id",
  "history": "string",
  "objective": "string",
  "assessment": "string",
  "plan": "string",
  "include_suppressed": false,
  "refset_id": "optional-loaded-refset-id"
}
```

All SOAP text fields default to an empty string. `refset_id`, when supplied, must match the loaded artefact.

## Observable Entity Request

`POST /v1/extract-observables`

```json
{
  "note_id": "optional-local-id",
  "objective": "BP 128/82. HR 76. RR 14. Sats 98%.",
  "include_suppressed": false,
  "refset_id": "optional-loaded-observations-refset-id"
}
```

The observable endpoint only accepts Objective text. It should be backed by an artefact built from the observations value set, for example refset `785380551000001102`.

## Examination Finding Request

`POST /v1/extract-examination-findings`

```json
{
  "note_id": "optional-local-id",
  "objective": "Chest clear on auscultation, no wheeze.",
  "include_suppressed": true,
  "refset_id": "optional-loaded-examination-findings-refset-id"
}
```

The examination findings endpoint only accepts Objective text. It should be backed by an artefact built from the examination findings value set, for example refset `932266131000001101`.

## Diagnosis Request

`POST /v1/extract-diagnoses`

```json
{
  "note_id": "optional-local-id",
  "assessment": "Asthma. ?Pneumonia.",
  "include_suppressed": true,
  "refset_id": "optional-loaded-diagnoses-refset-id"
}
```

The diagnosis endpoint only accepts Assessment text. It should be backed by an artefact built from the disorders/diagnoses value set, for example refset `782688301000001101`.

## Response

```json
{
  "note_id": "optional-local-id",
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
  "suppressed": [],
  "terminology_version": "http://snomed.info/sct/999000031000000106/version/20260201",
  "engine_version": "0.2.0",
  "ruleset_version": "ruleset-2026-06-12-v5",
  "artefact_hash": "sha256:...",
  "elapsed_micros": 900
}
```

The response shape is the same for all endpoints. For `/v1/extract-observables` and `/v1/extract-examination-findings`, every returned `field` is `objective`. For `/v1/extract-diagnoses`, every returned `field` is `assessment`. The `concept_id` values come from the endpoint's loaded artefact.

### Match fields

- `term_source` — provenance of the matched term: how it entered the artefact (`preferred_term`, `openehr-description-acronym`, `clinical_alias:<set>`, `built-in-observable-alias`, ...). Runtime morphology matches append `:morphology`, for example `openehr-display:morphology`. This replaces the former `confidence` field, which was a fixed per-field constant and **not** a probability. Do not rank or threshold on a probability the engine never produced.
- `value` — present on observable matches when a numeric value follows the label. Shape: `{ "text": "128/82", "unit": "mmHg", "span_start": 3, "span_end": 12 }`. `unit` is omitted when none was typed. The EPR can map this directly into an openEHR `DV_QUANTITY`/`DV_PROPORTION` rather than re-parsing the note. Value capture tolerates filler words, so `BP today 128/82` and `HR of 88 bpm` are captured; compact GP shorthand such as `afeb 37.2` is captured as body temperature when that concept is in the observable artefact.
- `assertion` — omitted for ordinary affirmed matches. The examination-finding endpoint can include normal and non-affirmed exam evidence in `matches`, currently `normal`, `negated`, or `uncertain`, so statements such as `HS normal` and `no goitre` are available to downstream templates without using `include_suppressed`.

`body_site` is present only on accepted symptom matches when the finding extractor
has been configured with a Body Site artefact and a nearby body-site mention can
safely enrich a broad symptom concept. Specific symptom concepts that already
imply the body site, such as earache, do not receive a duplicate `body_site`.

The Subjective field may be supplied as either `subjective` or `history` in `/v1/extract` requests.

## Suppressed Assertions

Suppressed matches are returned only when `include_suppressed` is true. A suppressed match has the same evidence fields as a positive match plus an assertion. Normal, negated, and uncertain examination findings are not treated as suppressed; they are returned in `matches` with their `assertion` because normality, absence, and equivocality are clinically meaningful in an exam template.

- `negated`
- `normal`
- `uncertain`
- `family_history`
- `historical_or_resolved`
- `hypothetical`
- `conditional`
- `planned`
- `non_patient`
- `ambiguous`

## Integration Rules

- The EPR must display candidates for clinician confirmation before adding codes to the record.
- `term_source` is provenance, not a score. There is no probability in the output to rank or threshold on.
- A captured `value` is still a candidate: confirm the observable and its value with the clinician before storing.
- Store confirmed clinician decisions separately from engine output.
- Keep the engine response with rule IDs and artefact hash for audit traceability.
