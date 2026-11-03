# Clinical Alias Sets

The extractor is deliberately conservative. Official synonyms should live in the enriched terminology export whenever possible. Alias files are a fallback for locally governed terms that are not present in the export.

## Bundled Patient Language

Both `build-openehr` and `build-rf2` now apply
[`patient-language-v1.json`](../aliases/patient-language-v1.json). This initial
set contains 50 explicit phrases across eight symptom concepts:

| Patient wording examples | Candidate symptom | SNOMED CT |
| --- | --- | --- |
| sore chest; chest hurts; chest feels sore | Chest pain | 29857009 |
| sore tummy; belly hurts; stomach ache | Abdominal pain | 21522001 |
| sore back; back aches | Backache | 161891005 |
| head hurts; head aches | Headache | 25064002 |
| feeling sick; feels queasy | Nausea | 422587007 |
| throwing up; threw up | Vomiting | 422400008 |
| hurts to pee; burning when weeing | Dysuria | 49650001 |
| out of breath; short of breath | Dyspnea | 267036007 |

These are local candidate mappings, not a claim that each phrase is an official
SNOMED description. Target IDs, active status and preferred terms were checked
against Ensemble's packaged UK symptoms value set dated 2026-02-01. Clinical
terminologist and CSO review remain outstanding; see [H-016](HAZARD_LOG.md#h-016--patient-language-maps-to-an-incorrect-clinical-concept).

The builder adds aliases only when their active concept is already in the input
value set. A different preferred term for the same target ID fails the build for
review. Aliases cannot add a missing concept, infer a disease from a symptom, or
create a code. Existing official synonyms take precedence over an alias that
would otherwise collide. Bare `sore`, `sick`, `poorly`, and `been sick` are not
mapped; the idiom `feeling sick of work` is excluded.

Every new alias keeps the original evidence span and carries
`clinical_alias:patient-language-v1` provenance, unless an existing terminology
variant already supplies the same phrase. The normal assertion and clinician
confirmation controls still apply. `No sore chest`, `possible sore chest`, and
`father has sore chest` must not become positive patient findings.

Rebuild and reload the terminology artefacts to enable the aliases. Upgrading
the executable alone does not enrich an old artefact; the new aliases are
included in the rebuilt artefact hash. Keep the alias file, engine/ruleset
version, input terminology version and artefact hash together in release
evidence.

For new patient phrases, add an explicit concept-bound entry or term to this
file, verify the target in the licensed source, and add positive, context and
ambiguity regression examples in [`tests/patient_language.rs`](../tests/patient_language.rs).
Validate against the complete deployed value set as well as small fixtures,
because competing official synonyms can change the result. Do not add a
global `sore` → `pain` replacement or an unreviewed spelling/semantic guess.

The opt-in full-terminology regression runs every bundled phrase through six
assertion contexts:

```sh
SNOMED_SYMPTOMS_VALUESET=/path/to/symptoms.json cargo test --locked --test patient_language -- --include-ignored
```

Ensemble's current integration also applies `clinical-list-answers.patch`,
which changes `RULESET_VERSION`. When adopting this upstream release, refresh
that patch's version hunk, pin the new upstream commit, rebuild the artefacts,
and run the combined integration regressions. This change does not update or
deploy the consuming EPR.

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

Observable labels have an extra safety gate. Short labels such as `P` from `Pulse rate` or `T` from `BT - Body temperature` are accepted only when followed by a numeric value, and ordinary prose remains subject to the normal ambiguity checks.

## Runtime Shorthand Expansion

Some UK general-practice shorthand is expanded by the engine itself before matching, so it does not depend on an alias file or a particular terminology description being present:

`c/o`, `o/e`, `h/o`, `d&v`, `n&v`, `sob`, `soboe`, `doe`, `fh`/`fhx`, `pmh`/`pmhx`, `hx`, `nad`.

Expansion runs on the SOAP text only — terminology patterns are never expanded — and every expanded byte keeps a map back to the original shorthand, so a match's reported span still points at the characters the clinician typed. Expansion also feeds the assertion engine: `fhx diabetes` becomes a family-history frame, and `h/o depression` a historical one. `fh` is left unexpanded in the Objective field, where it denotes fundal height or fetal heart rather than family history.

Alias files remain the route for locally governed term synonyms; runtime expansion only covers field-structuring shorthand that is stable across GP practice.
