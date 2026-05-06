# Validation Plan

## Goals

The first production gate is safety-first extraction, not broad recall.

Required targets:

- zero known critical false-positive failures for negated, family-history, non-patient, uncertain, conditional, historical, or planned mentions;
- at least 98% positive predictive value on clinician-reviewed validation data;
- p95 extraction latency under 15 ms for a typical SOAP note on agreed production hardware;
- every accepted or suppressed match has a span, rule ID, explanation, artefact hash, and terminology version.

## Dataset Layers

1. Unit fixtures: tiny fake terminology, deterministic edge cases.
2. Synthetic corpus: generated from the loaded artefact with labelled affirmed and suppressed scenarios.
3. Clinician-reviewed golden set: de-identified SOAP notes sampled from real workflows.
4. Incident regression set: every safety incident becomes a permanent test.

## Synthetic Scenarios

For each selected concept, generate:

- affirmed assessment mention;
- negated history mention;
- uncertain/query mention;
- family-history mention;
- historical/resolved mention;
- conditional/hypothetical mention;
- planned/screening mention.

Hard negatives must include:

- `no chest pain`
- `denies diabetes`
- `father had MI`
- `?asthma`
- `rule out pneumonia`
- `screen for depression`

## Review Process

Each validation run must record:

- engine version;
- ruleset version;
- terminology artefact hash;
- dataset id/hash;
- false positives;
- false negatives;
- latency summary;
- reviewer and date.

Clinician review should classify false positives by hazard category, not just by generic precision metrics.
