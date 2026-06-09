# DCB0129 Clinical Safety Case Outline

## Clinical Safety Statement

The extractor proposes candidate SNOMED CT finding, observable entity, examination finding, and diagnosis/disorder codes for clinician confirmation. It must not silently add findings, observations, or diagnoses to the patient record.

## Safety Roles

Required before clinical deployment:

- Clinical Safety Officer;
- clinical terminologist;
- responsible software owner;
- EPR integration owner;
- information governance owner.

## Hazard Controls

Primary controls:

- refset-bounded concept scope;
- deterministic matching;
- conservative assertion suppression;
- clinician confirmation workflow;
- rule-level explanations;
- artefact and ruleset versioning;
- regression testing from incidents.

## Deployment Preconditions

Before deployment:

- complete hazard log review;
- complete technical file;
- pass validation gates;
- confirm UK medical device classification;
- verify EPR UI clearly separates suggestions from confirmed codes;
- verify raw note text is not retained by extractor logs.

## Post-Market Surveillance

Collect:

- false-positive reports;
- false-negative reports;
- clinician override rates;
- latency and error metrics;
- terminology update issues;
- integration incidents.

Each safety incident must create a tracked change or an explicit risk acceptance.
