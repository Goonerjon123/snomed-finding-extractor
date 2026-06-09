# Terminology And Licensing

## Source Inputs

The production terminology path is:

1. UK SNOMED CT RF2 Snapshot from TRUD.
2. A supplied refset or openEHR value set manifest limiting concepts to the intended clinical scope.
3. A generated runtime artefact containing only the terms needed by the extractor.

The supplied current finding value set path is:

```text
D:\SnoBehr\Refsets for Export\symptoms-20260201.openehr-valueset.json
```

The file inspected on 6 May 2026 is an openEHR terminology value set binding manifest with:

- id: `symptoms`
- refset id: `238873041000001104`
- terminology release date: `20260201`
- active member count: `128202`
- member shape: `code`, `display`, `fsn`, `active`, `system`

The supplied current observable entity value set path is:

```text
D:\SnoBehr\Refsets for Export\Observables\observations-20260201.openehr-valueset.json
```

The file inspected on 6 May 2026 is an openEHR terminology value set binding manifest with:

- id: `observations`
- refset id: `785380551000001102`
- terminology release date: `20260201`
- active member count: `6362`
- member shape: `code`, `display`, `fsn`, `active`, `system`, `descriptions`

The supplied current examination findings value set path is:

```text
D:\SnoBehr\Refsets for Export\Examination Findings\gp-approved-exam-findings-20260201.openehr-valueset.json
```

The file inspected on 1 June 2026 is an openEHR terminology value set binding manifest with:

- id: `examination-findings`
- refset id: `932266131000001101`
- terminology release date: `20260201`
- member shape: `code`, `display`, `fsn`, `active`, `system`, `descriptions`

The supplied current diagnosis/disorder value set path is:

```text
D:\SnoBehr\Refsets for Export\Disorders\diagnoses-20260201.openehr-valueset.json
```

The file inspected on 9 June 2026 is an openEHR terminology value set binding manifest with:

- id: `diagnoses-5`
- refset id: `782688301000001101`
- terminology release date: `20260201`
- semantic scope: all active SNOMED CT concepts whose fully specified name semantic tag is `(disorder)`
- member shape: `code`, `display`, `fsn`, `active`, `system`, `descriptions`

## Repository Rule

Do not commit:

- TRUD RF2 zip files;
- RF2 extracted text files;
- generated production artefacts derived from licensed SNOMED content;
- large value set exports unless licensing and repository visibility have been approved.

The repo includes only tiny synthetic fixtures with fake concept IDs for tests.

## Artefact Content

The generated artefact contains:

- schema version;
- SNOMED terminology version/source release;
- refset id;
- concept id;
- preferred term;
- active status;
- descriptions, synonyms, description IDs, and acceptability from enriched openEHR exports or RF2 language data;
- runtime variants;
- built-in observable aliases for common Objective abbreviations when the matching observable concept is present in the artefact;
- reviewed clinical aliases, when supplied at build time as a fallback for content not present in the export;
- artefact hash.

## Build Modes

`build-openehr` uses the supplied manifest directly. When the manifest includes active member descriptions, the importer uses preferred terms, synonyms, description IDs, and acceptability to build runtime variants.

`build-rf2` uses RF2 concept, description, language, and refset snapshot files. It should become the production build path because it can include active descriptions, synonyms, and UK language acceptability.

Both build paths still accept an optional alias file, but this should be a fallback for locally governed content not present in the enriched terminology export.

## Change Control

Every new terminology artefact must record:

- source file names and hashes;
- SNOMED edition/version;
- refset id/version;
- artefact hash;
- validation report;
- clinical reviewer sign-off.
