# API Contract

## Intended Use

The engine supports clinicians by identifying candidate SNOMED CT finding codes mentioned in SOAP free text for clinician review and confirmation.

## Request

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

## Response

```json
{
  "note_id": "optional-local-id",
  "matches": [
    {
      "concept_id": "1000000001",
      "preferred_term": "Chest pain",
      "field": "assessment",
      "span_start": 0,
      "span_end": 10,
      "matched_text": "Chest pain",
      "normalized_match": "chest pain",
      "confidence": 0.97,
      "rule_ids": ["ASSERT_AFFIRMED_PATIENT_FINDING"],
      "explanation": "Accepted as an affirmed patient finding in the assessment field; no suppression rule fired."
    }
  ],
  "suppressed": [],
  "terminology_version": "http://snomed.info/sct/999000031000000106/version/20260201",
  "engine_version": "0.1.0",
  "ruleset_version": "ruleset-2026-05-06-v1",
  "artefact_hash": "sha256:...",
  "elapsed_micros": 900
}
```

## Suppressed Assertions

Suppressed matches are returned only when `include_suppressed` is true. A suppressed match has the same evidence fields as a positive match plus an assertion:

- `negated`
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
- The EPR must not treat `confidence` as probability. It is a deterministic ranking hint based on field and rule outcome.
- Store confirmed clinician decisions separately from engine output.
- Keep the engine response with rule IDs and artefact hash for audit traceability.
