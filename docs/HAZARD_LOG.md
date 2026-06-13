# Hazard Log

| ID | Hazard | Cause | Harm | Initial Severity | Controls | Residual Status |
| --- | --- | --- | --- | --- | --- | --- |
| H-001 | Negated finding is proposed as positive | Negation scope failure or missed coordinated phrase member | Incorrect coded problem list or downstream decision support | High | Clause-scoped negation binding (bounded gap allow-list, contrast breakers, coordinated-sibling propagation), coordinated shared-head matching before assertion classification, regression corpus, clinician confirmation | Open |
| H-002 | Family-history finding is proposed as patient finding | Experiencer detection failure | Incorrect patient diagnosis | High | Experiencer binding through possessive connectors before/after the match (social and reporting mentions excluded), regression corpus, clinician confirmation | Open |
| H-003 | Planned screening/test target is proposed as current finding | Plan field or action cue failure | Incorrect morbidity coding | High | Plan field suppressed by default; completed-action override scoped to nearest cue with advice-style blockers; planned-action rules; regression corpus | Open |
| H-004 | Ambiguous abbreviation is matched incorrectly | Common acronym overlap | Incorrect code suggestion | Medium | Ambiguous term blocklist, explicit allow flag, acronym specificity guard | Open |
| H-005 | Out-of-date terminology used | Artefact not refreshed | Missing or stale codes | Medium | Artefact version/hash display, release procedure | Open |
| H-006 | Raw patient text appears in logs | Integration logging error | Confidentiality breach | High | No raw text logging in engine, integration logging review | Open |
| H-007 | Clinician assumes suggestion is confirmed code | Poor UI distinction | Incorrect record entry | High | EPR UI requirement, usability review, confirmation step | Open |
| H-008 | Low recall misses relevant findings | Conservative matching | Manual coding burden or missed opportunity | Medium | Report false negatives, clinician can manually code | Open |
| H-009 | Objective abbreviation maps to wrong observable entity | Ambiguous vital-sign shorthand | Incorrect observation code suggestion | Medium | Built-in alias whitelist, numeric-value requirement for very short labels, refset-bounded matching, clinician confirmation, alias regression tests | Open |
| H-010 | Examination phrase maps to wrong finding | Local shorthand or modifier-heavy examination wording is interpreted too broadly | Incorrect examination finding suggestion | Medium | Refset-bounded matching, terminology-derived context trimming only from official descriptions, bounded body-site modifier matching, suppression rules, clinician confirmation | Open |
| H-011 | Query or differential diagnosis proposed as confirmed diagnosis | Assessment text contains uncertainty cues | Incorrect diagnosis/problem code suggestion | High | Assessment-only diagnosis endpoint, uncertainty suppression, clinician confirmation, diagnosis regression tests | Open |

| H-012 | Captured observable value bound to the wrong observable | Value-capture picks up an unrelated number after the label | Incorrect quantity proposed for confirmation | Medium | Value capture limited to the observables extraction; bounded filler tolerance; numeric-token requirement; known-unit allow-list; original-text spans; clinician confirmation of value before storage | Open |
| H-013 | Ambiguous term silently removed, reducing recall | Ambiguity guard drops a multi-concept term | Missed candidate code, manual coding burden | Low | Build-time ambiguity audit (`audit-terms`, startup warning) surfaces dropped terms for terminology review | Open |

Severity and residual risk must be reviewed by the Clinical Safety Officer before clinical deployment.
