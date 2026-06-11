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

Decision: matches found in the Plan field are suppressed unless a completed therapeutic action asserts them.

Rationale: Plan text often contains screening, referrals, tests, and differential diagnoses rather than asserted current findings. The exception (ADR-006) recovers genuinely-asserted content such as "Started amoxicillin for LRTI" without admitting "Screen for depression".

## ADR-005: Clause-Scoped Context Over Sentence-Wide Cues

Decision: negation, uncertainty, planned, historical, and experiencer cues bind to a match only when the tokens between cue and match are descriptors, digits, or coordinated sibling matches; affirmative verbs, unrelated nouns, and contrast words break the binding.

Rationale: sentence-wide cue scanning over-suppressed common GP phrasing ("no improvement in cough", "no fever, has cough", "lives with his mother and reports chest pain"). Clause-scoped binding preserves recall while keeping the safety-critical suppressions, including coordination ("no cough or wheeze" suppresses both). The scope rules are deterministic and unit-tested as a regression corpus.

## ADR-006: Plan-Field Completed-Action Override

Decision: a completed-action cue ("started", "commenced", "given", "prescribed", ...) that is the nearest cue before a Plan-field match asserts that match (rule `PLAN_COMPLETED_ACTION`). Nearer planned, conditional, uncertainty, or negation cues, and advice-style actions, still suppress.

Rationale: a treatment started in the Plan implies the condition it treats. Scoping the override to the nearest cue keeps "Started celecoxib, monitor for GI bleed" suppressed on the monitored target.

## ADR-007: Provenance Instead Of Synthetic Confidence

Decision: replace the fixed per-field `confidence` constant with a `term_source` provenance string and, for observables, a captured `value`/`unit`.

Rationale: the constant was not a probability and invited unsafe thresholding. Provenance is honest about why a term matched; the captured value lets the EPR fill an openEHR quantity without re-parsing the note.
