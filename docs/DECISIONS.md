# Architecture Decisions

## ADR-001: Rust Core With JSON Boundary

Decision: implement the extractor as a Rust library with CLI and optional HTTP sidecar.

Rationale: Rust gives predictable latency, low memory use, and portable integration through JSON while avoiding runtime ML dependencies.

## ADR-002: Refset-Bounded Extraction

Decision: v1 only emits concepts present in the loaded artefact/refset.

Rationale: a bounded concept set is easier to review, validate, explain, and govern as a medical device component.

## ADR-003: Deterministic Runtime

Decision: no runtime ML in v1.

Rationale: deterministic rules provide clearer traceability and safer change control. ML may assist offline synthetic dataset generation or term review, but generated outputs must be reviewed before release.

## ADR-004: Plan Field Suppressed By Default

Decision: matches found in the Plan field are suppressed unless a future controlled ruleset explicitly permits them.

Rationale: Plan text often contains screening, referrals, tests, and differential diagnoses rather than asserted current findings.
