# Clinical Alias Sets

The extractor is deliberately conservative. Official synonyms should live in the enriched terminology export whenever possible. Alias files are a fallback for locally governed terms that are not present in the export.

## Alias File Shape

```json
{
  "schema_version": 1,
  "name": "symptoms-gp-aliases",
  "aliases": [
    {
      "concept_id": "267036007",
      "expected_preferred_term": "Dyspnea",
      "terms": ["short of breath", "shortness of breath", "breathless"],
      "source": "gp-breathlessness-v1",
      "allow_ambiguous": true
    }
  ]
}
```

`expected_preferred_term` is optional but recommended. It makes the build fail if a future terminology artefact maps the concept ID to an unexpected display term.

`allow_ambiguous` should be true only for clinically reviewed short acronyms such as `SOB`. The engine still applies word boundaries, longest-match logic, negation, family-history, and Plan-field suppression.

## Breathlessness

The current symptoms export includes official descriptions for breathlessness, including:

- `Breathlessness`, `SOB - Shortness of breath`, and `Shortness of breath` for SNOMED `267036007` / `Dyspnea`.
- `SOBOE - Shortness of breath on exertion` and `Short of breath on exertion` for SNOMED `60845006` / `Dyspnea on exertion`.

The importer also derives safe runtime variants from those official descriptions: leading acronyms such as `SOB` and `SOBOE`, phrase variants such as `short of breath` from `Shortness of breath`, simple diagnosis shorthand such as `Type 2 diabetes` from `Type 2 diabetes mellitus`, morphology variants such as `swollen left tonsil` from `Swelling of left tonsil`, and examination phrase variants such as `Chest clear` from `Chest clear on auscultation`.

Derived acronym prefixes are still safety-gated. For example, the importer can derive `URTI` from a base description such as `URTI - Infection of the upper respiratory tract`, but it will not derive the bare acronym from a more specific description such as `URTI - Viral upper respiratory tract infection`, because that would silently add a viral qualifier that the clinician did not type.

Observable labels have an extra safety gate. Short labels such as `P` from `Pulse rate` are accepted only when followed by a numeric value, and ordinary prose remains subject to the normal ambiguity checks.
