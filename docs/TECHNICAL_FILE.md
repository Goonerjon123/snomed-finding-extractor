# Technical File Outline

## Intended Purpose

Supports clinicians by identifying candidate SNOMED CT finding codes mentioned in SOAP free text and candidate SNOMED CT observable entity codes mentioned in the Objective field for clinician review and confirmation.

The engine is not intended to diagnose, triage, recommend treatment, or automatically code the medical record without clinician confirmation.

## Software Description

- Rust deterministic runtime.
- Offline terminology artefact builder.
- SOAP-aware extraction API.
- Objective-only observable entity extraction API.
- Optional HTTP sidecar.
- Synthetic validation and evaluation tooling.

## Architecture Evidence

Maintain:

- architecture diagram;
- data flow diagram;
- threat model;
- API contract;
- terminology build procedure;
- traceability matrix from requirements to tests;
- benchmark reports.

## Verification Evidence

Maintain:

- unit test reports;
- integration test reports;
- synthetic corpus evaluation;
- clinician-reviewed validation report;
- latency benchmark report;
- release checklist.

## Clinical Evaluation

Document:

- intended users and workflow;
- clinical claims;
- validation dataset composition;
- acceptance thresholds;
- failure analysis;
- residual risk.

## Change Control

Controlled changes include:

- new runtime ML;
- expanded concept scope;
- new refsets;
- assertion rule changes;
- moving from clinician confirmation to auto-coding;
- new host system integration patterns.
