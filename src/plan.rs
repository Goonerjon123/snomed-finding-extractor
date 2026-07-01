use crate::model::{
    PlanEntityKind, PlanEntityMatch, PlanExtractRequest, PlanExtractResponse, SoapField,
};
use crate::normalization::{normalize_clinical_text, NormalizedText};
use crate::{ENGINE_VERSION, RULESET_VERSION};
use std::collections::HashSet;
use std::time::Instant;

pub fn extract_plan_entities(request: PlanExtractRequest) -> PlanExtractResponse {
    let started = Instant::now();
    let mut matches = Vec::new();

    if !request.plan.trim().is_empty() {
        let normalized = normalize_clinical_text(&request.plan, SoapField::Plan);
        let tokens = tokenize(&normalized);
        let context = PlanMatchContext {
            original: &request.plan,
            normalized: &normalized,
            tokens: &tokens,
        };

        add_prescription_matches(&mut matches, &context);
        add_phrase_matches(
            &mut matches,
            &context,
            PlanEntityKind::Referral,
            "PLAN_REFERRAL_CUE",
            REFERRAL_PHRASES,
            ContextPolicy::Standard,
        );
        add_emed3_matches(&mut matches, &context);
        add_appointment_matches(&mut matches, &context);
        add_phrase_matches(
            &mut matches,
            &context,
            PlanEntityKind::Investigation,
            "PLAN_INVESTIGATION_CUE",
            INVESTIGATION_PHRASES,
            ContextPolicy::Standard,
        );
        add_phrase_matches(
            &mut matches,
            &context,
            PlanEntityKind::Procedure,
            "PLAN_PROCEDURE_CUE",
            PROCEDURE_PHRASES,
            ContextPolicy::Standard,
        );
        add_phrase_matches(
            &mut matches,
            &context,
            PlanEntityKind::Monitoring,
            "PLAN_MONITORING_CUE",
            MONITORING_PHRASES,
            ContextPolicy::Standard,
        );
        add_phrase_matches(
            &mut matches,
            &context,
            PlanEntityKind::MedicationReview,
            "PLAN_MEDICATION_REVIEW_CUE",
            MEDICATION_REVIEW_PHRASES,
            ContextPolicy::Standard,
        );
        add_phrase_matches(
            &mut matches,
            &context,
            PlanEntityKind::AdministrativeTask,
            "PLAN_ADMINISTRATIVE_TASK_CUE",
            ADMINISTRATIVE_TASK_PHRASES,
            ContextPolicy::Standard,
        );
    }

    dedupe_plan_matches(&mut matches);
    matches.sort_by_key(|item| (item.span_start, item.span_end, item.entity));

    let mut seen_entities = HashSet::new();
    let plan_entities = matches
        .iter()
        .filter_map(|item| seen_entities.insert(item.entity).then_some(item.entity))
        .collect::<Vec<_>>();

    PlanExtractResponse {
        note_id: request.note_id,
        plan_entities,
        matches,
        engine_version: ENGINE_VERSION.to_string(),
        ruleset_version: RULESET_VERSION.to_string(),
        elapsed_micros: started.elapsed().as_micros(),
    }
}

#[derive(Debug, Clone)]
struct PlanToken {
    text: String,
    normalized_start: usize,
    normalized_end: usize,
    original_start: usize,
    original_end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextPolicy {
    Standard,
    AllowInternalNegation,
}

#[derive(Debug, Clone, Copy)]
struct PlanMatchContext<'a> {
    original: &'a str,
    normalized: &'a NormalizedText,
    tokens: &'a [PlanToken],
}

const PRESCRIPTION_PHRASES: &[&str] = &[
    "prescription",
    "prescriptions",
    "prescribe",
    "prescribed",
    "issue prescription",
    "issued prescription",
    "acute prescription",
    "repeat prescription",
    "e prescribe",
    "e prescribed",
    "rx",
    "script",
];

const REFERRAL_PHRASES: &[&str] = &[
    "refer",
    "refers",
    "referred",
    "referral",
    "onward referral",
    "onwards referral",
    "secondary care referral",
    "secondary care",
    "mental health referral",
    "physio referral",
    "physiotherapy referral",
    "private referral",
    "insurance referral",
    "urgent cancer referral",
    "urgent cancer pathway",
    "two week wait",
    "2ww",
];

const EMED3_PHRASES_STANDARD: &[&str] = &[
    "emed3",
    "e med3",
    "med3",
    "med 3",
    "fit note",
    "fitness note",
    "sick note",
    "statement of fitness",
    "fitness for work",
    "off work",
    "time off work",
    "take time off",
    "stay off",
    "signed off",
    "sign off",
];

const EMED3_PHRASES_ALLOW_NEGATION: &[&str] = &["not fit for work"];

const APPOINTMENT_PHRASES: &[&str] = &[
    "book appointment",
    "book an appointment",
    "booked appointment",
    "arrange appointment",
    "arranged appointment",
    "appointment with",
    "appt with",
    "book review",
    "booked review",
    "arrange review",
    "arranged review",
    "follow up",
    "followup",
    "f u",
];

const INVESTIGATION_PHRASES: &[&str] = &[
    "blood test",
    "blood tests",
    "bloods",
    "urine dip",
    "urine dipstick",
    "urine test",
    "urine sample",
    "msu",
    "ecg",
    "x ray",
    "xray",
    "ultrasound",
    "ct",
    "mri",
    "fit test",
    "spirometry",
    "swab",
    "stool sample",
    "sputum culture",
    "culture",
    "fbc",
    "u e",
    "lft",
    "tft",
    "hba1c",
    "crp",
    "esr",
];

const PROCEDURE_PHRASES: &[&str] = &[
    "wound dressing",
    "dressing change",
    "suture removal",
    "remove sutures",
    "ear irrigation",
    "ear syringing",
    "joint injection",
    "steroid injection",
    "depot injection",
    "coil insertion",
    "coil removal",
    "implant insertion",
    "implant removal",
    "cryotherapy",
    "minor surgery",
];

const MONITORING_PHRASES: &[&str] = &[
    "bp diary",
    "blood pressure diary",
    "home bp",
    "home blood pressure",
    "home readings",
    "peak flow diary",
    "symptom diary",
    "glucose monitoring",
    "blood sugar monitoring",
    "weight check",
    "monitor symptoms",
    "monitor bp",
    "monitor blood pressure",
    "monitor for",
    "repeat bp",
    "repeat blood pressure",
    "daily weights",
];

const MEDICATION_REVIEW_PHRASES: &[&str] = &[
    "medication review",
    "medicines review",
    "drug review",
    "review medication",
    "review medications",
    "review meds",
    "review current medication",
    "review regular medication",
    "review repeat medication",
    "review repeats",
    "review dose",
    "dose review",
    "increase dose",
    "reduce dose",
    "stop medication",
    "stop meds",
    "withhold medication",
    "change medication",
    "switch medication",
    "titrate",
    "deprescribe",
    "deprescribing",
];

const ADMINISTRATIVE_TASK_PHRASES: &[&str] = &[
    "letter",
    "letters",
    "form",
    "forms",
    "report",
    "reports",
    "certificate",
    "certificates",
    "dvla",
    "housing letter",
    "insurance paperwork",
    "insurance form",
    "travel form",
    "esa form",
    "pip form",
    "blue badge",
];

fn add_prescription_matches(matches: &mut Vec<PlanEntityMatch>, context: &PlanMatchContext<'_>) {
    add_phrase_matches(
        matches,
        context,
        PlanEntityKind::Prescription,
        "PLAN_PRESCRIPTION_CUE",
        PRESCRIPTION_PHRASES,
        ContextPolicy::Standard,
    );
    add_medication_action_matches(matches, context);
    add_medication_dose_matches(matches, context);
}

fn add_emed3_matches(matches: &mut Vec<PlanEntityMatch>, context: &PlanMatchContext<'_>) {
    add_phrase_matches(
        matches,
        context,
        PlanEntityKind::Emed3,
        "PLAN_EMED3_CUE",
        EMED3_PHRASES_STANDARD,
        ContextPolicy::Standard,
    );
    add_phrase_matches(
        matches,
        context,
        PlanEntityKind::Emed3,
        "PLAN_EMED3_CUE",
        EMED3_PHRASES_ALLOW_NEGATION,
        ContextPolicy::AllowInternalNegation,
    );
}

fn add_appointment_matches(matches: &mut Vec<PlanEntityMatch>, context: &PlanMatchContext<'_>) {
    add_phrase_matches(
        matches,
        context,
        PlanEntityKind::Appointment,
        "PLAN_APPOINTMENT_CUE",
        APPOINTMENT_PHRASES,
        ContextPolicy::Standard,
    );

    for index in 0..context.tokens.len() {
        let token = context.tokens[index].text.as_str();
        if !matches!(
            token,
            "review" | "rv" | "see" | "seen" | "follow" | "appointment" | "appt"
        ) {
            continue;
        }

        let Some(end_index) = definite_appointment_end(context.tokens, context.original, index)
        else {
            continue;
        };
        push_plan_match(
            matches,
            context,
            index,
            end_index,
            PlanEntityKind::Appointment,
            "PLAN_APPOINTMENT_CUE",
            ContextPolicy::Standard,
        );
    }
}

fn add_medication_action_matches(
    matches: &mut Vec<PlanEntityMatch>,
    context: &PlanMatchContext<'_>,
) {
    for index in 0..context.tokens.len() {
        if !is_prescription_action(context.tokens[index].text.as_str()) {
            continue;
        }

        let mut current = index + 1;
        while current < context.tokens.len() && current <= index + 7 {
            if original_gap_has_hard_boundary(
                context.original,
                context.tokens[index].original_end,
                context.tokens[current].original_start,
            ) {
                break;
            }
            if is_medication_object(context.tokens[current].text.as_str()) {
                push_plan_match(
                    matches,
                    context,
                    index,
                    current + 1,
                    PlanEntityKind::Prescription,
                    "PLAN_PRESCRIPTION_MEDICATION_ACTION",
                    ContextPolicy::Standard,
                );
                break;
            }
            if !is_prescription_filler(context.tokens[current].text.as_str()) {
                break;
            }
            current += 1;
        }
    }
}

fn add_medication_dose_matches(matches: &mut Vec<PlanEntityMatch>, context: &PlanMatchContext<'_>) {
    for index in 0..context.tokens.len() {
        if !is_medication_object(context.tokens[index].text.as_str()) {
            continue;
        }

        let mut end_index = None;
        let mut current = index + 1;
        while current < context.tokens.len() && current <= index + 4 {
            if original_gap_has_hard_boundary(
                context.original,
                context.tokens[index].original_end,
                context.tokens[current].original_start,
            ) {
                break;
            }
            if is_dose_or_frequency(context.tokens[current].text.as_str()) {
                end_index = Some(current + 1);
            } else if !matches!(
                context.tokens[current].text.as_str(),
                "for" | "to" | "x" | "a" | "an" | "the" | "and"
            ) {
                break;
            }
            current += 1;
        }

        if let Some(end_index) = end_index {
            push_plan_match(
                matches,
                context,
                index,
                end_index,
                PlanEntityKind::Prescription,
                "PLAN_PRESCRIPTION_MEDICATION_DOSE",
                ContextPolicy::Standard,
            );
        }
    }
}

fn add_phrase_matches(
    matches: &mut Vec<PlanEntityMatch>,
    context: &PlanMatchContext<'_>,
    entity: PlanEntityKind,
    rule_id: &'static str,
    phrases: &[&str],
    policy: ContextPolicy,
) {
    for phrase in phrases {
        let phrase_tokens = phrase.split_whitespace().collect::<Vec<_>>();
        if phrase_tokens.is_empty() {
            continue;
        }
        for start_index in 0..context.tokens.len() {
            if tokens_match_phrase(context.tokens, start_index, &phrase_tokens) {
                push_plan_match(
                    matches,
                    context,
                    start_index,
                    start_index + phrase_tokens.len(),
                    entity,
                    rule_id,
                    policy,
                );
            }
        }
    }
}

fn push_plan_match(
    matches: &mut Vec<PlanEntityMatch>,
    context: &PlanMatchContext<'_>,
    start_index: usize,
    end_index: usize,
    entity: PlanEntityKind,
    rule_id: &'static str,
    policy: ContextPolicy,
) {
    if start_index >= end_index || end_index > context.tokens.len() {
        return;
    }
    if context_blocks_match(
        policy,
        context.tokens,
        context.original,
        start_index,
        end_index,
    ) {
        return;
    }

    let start_token = &context.tokens[start_index];
    let end_token = &context.tokens[end_index - 1];
    let span_start = start_token.original_start;
    let span_end = end_token.original_end;
    let normalized_start = start_token.normalized_start;
    let normalized_end = end_token.normalized_end;

    matches.push(PlanEntityMatch {
        entity,
        field: SoapField::Plan,
        span_start,
        span_end,
        matched_text: context.original[span_start..span_end].to_string(),
        normalized_match: context.normalized.text[normalized_start..normalized_end].to_string(),
        rule_ids: vec![rule_id.to_string()],
        explanation: format!(
            "Accepted as a Plan entity: {} cue in the Plan field.",
            entity.label()
        ),
    });
}

fn tokenize(normalized: &NormalizedText) -> Vec<PlanToken> {
    let mut tokens = Vec::new();
    let mut token_start = None;

    for (idx, ch) in normalized.text.char_indices() {
        if ch == ' ' {
            if let Some(start) = token_start.take() {
                push_token(&mut tokens, normalized, start, idx);
            }
        } else if token_start.is_none() {
            token_start = Some(idx);
        }
    }

    if let Some(start) = token_start {
        push_token(&mut tokens, normalized, start, normalized.text.len());
    }

    tokens
}

fn push_token(tokens: &mut Vec<PlanToken>, normalized: &NormalizedText, start: usize, end: usize) {
    let Some((original_start, original_end)) = normalized.original_range(start, end) else {
        return;
    };
    tokens.push(PlanToken {
        text: normalized.text[start..end].to_string(),
        normalized_start: start,
        normalized_end: end,
        original_start,
        original_end,
    });
}

fn tokens_match_phrase(tokens: &[PlanToken], start_index: usize, phrase: &[&str]) -> bool {
    start_index + phrase.len() <= tokens.len()
        && phrase
            .iter()
            .enumerate()
            .all(|(offset, expected)| tokens[start_index + offset].text == *expected)
}

fn context_blocks_match(
    policy: ContextPolicy,
    tokens: &[PlanToken],
    original: &str,
    start_index: usize,
    end_index: usize,
) -> bool {
    let previous_start = start_index.saturating_sub(4);
    for token in &tokens[previous_start..start_index] {
        if is_preceding_blocker(token.text.as_str()) || is_uncertain_blocker(token.text.as_str()) {
            return true;
        }
    }

    for token in &tokens[start_index..end_index] {
        if is_uncertain_blocker(token.text.as_str()) {
            return true;
        }
        if is_preceding_blocker(token.text.as_str())
            && !(policy == ContextPolicy::AllowInternalNegation && token.text == "not")
        {
            return true;
        }
    }

    let after_end = (end_index + 5).min(tokens.len());
    for token in &tokens[end_index..after_end] {
        if original_gap_has_hard_boundary(
            original,
            tokens[end_index - 1].original_end,
            token.original_start,
        ) {
            break;
        }
        if token.text == "if" || token.text == "unless" {
            return true;
        }
    }

    false
}

fn definite_appointment_end(
    tokens: &[PlanToken],
    original: &str,
    start_index: usize,
) -> Option<usize> {
    let mut current = start_index + 1;
    let mut end_index = None;
    while current < tokens.len() && current <= start_index + 8 {
        if original_gap_has_hard_boundary(
            original,
            tokens[start_index].original_end,
            tokens[current].original_start,
        ) {
            break;
        }
        if tokens[current].text == "if" || tokens[current].text == "unless" {
            return None;
        }
        if is_appointment_marker(tokens[current].text.as_str()) {
            end_index = Some(current + 1);
        } else if end_index.is_some() {
            break;
        }
        current += 1;
    }
    end_index
}

fn is_prescription_action(token: &str) -> bool {
    matches!(
        token,
        "start"
            | "started"
            | "commence"
            | "commenced"
            | "begin"
            | "began"
            | "trial"
            | "give"
            | "given"
            | "issue"
            | "issued"
            | "provide"
            | "provided"
            | "prescribe"
            | "prescribed"
    )
}

fn is_prescription_filler(token: &str) -> bool {
    token.chars().all(|ch| ch.is_ascii_digit())
        || matches!(
            token,
            "a" | "an"
                | "the"
                | "patient"
                | "pt"
                | "him"
                | "her"
                | "them"
                | "on"
                | "with"
                | "regular"
                | "oral"
                | "topical"
                | "short"
                | "course"
                | "of"
                | "new"
                | "trial"
                | "supply"
                | "rescue"
                | "for"
                | "days"
                | "day"
        )
}

fn is_medication_object(token: &str) -> bool {
    matches!(
        token,
        "antibiotic"
            | "antibiotics"
            | "amoxicillin"
            | "amoxil"
            | "amoxiclav"
            | "penicillin"
            | "phenoxymethylpenicillin"
            | "flucloxacillin"
            | "doxycycline"
            | "clarithromycin"
            | "erythromycin"
            | "nitrofurantoin"
            | "trimethoprim"
            | "analgesia"
            | "analgesic"
            | "analgesics"
            | "paracetamol"
            | "ibuprofen"
            | "naproxen"
            | "codeine"
            | "morphine"
            | "inhaler"
            | "salbutamol"
            | "steroid"
            | "steroids"
            | "prednisolone"
            | "antihistamine"
            | "cetirizine"
            | "loratadine"
            | "ppi"
            | "omeprazole"
            | "lansoprazole"
            | "statin"
            | "atorvastatin"
            | "simvastatin"
            | "antidepressant"
            | "sertraline"
            | "fluoxetine"
            | "citalopram"
            | "mirtazapine"
            | "contraceptive"
            | "pill"
            | "insulin"
            | "metformin"
            | "ramipril"
            | "amlodipine"
            | "bendroflumethiazide"
            | "anticoagulant"
            | "apixaban"
            | "rivaroxaban"
            | "warfarin"
            | "laxative"
            | "cream"
            | "ointment"
            | "drops"
            | "spray"
    )
}

fn is_dose_or_frequency(token: &str) -> bool {
    token.chars().any(|ch| ch.is_ascii_digit())
        || matches!(
            token,
            "mg" | "mcg"
                | "g"
                | "ml"
                | "bd"
                | "tds"
                | "qds"
                | "od"
                | "daily"
                | "nocte"
                | "mane"
                | "prn"
                | "days"
                | "day"
        )
}

fn is_appointment_marker(token: &str) -> bool {
    token.chars().all(|ch| ch.is_ascii_digit())
        || matches!(
            token,
            "in" | "within"
                | "next"
                | "with"
                | "by"
                | "at"
                | "gp"
                | "doctor"
                | "nurse"
                | "pharmacist"
                | "clinician"
                | "clinic"
                | "appointment"
                | "appt"
                | "book"
                | "booked"
                | "arrange"
                | "arranged"
                | "scheduled"
                | "week"
                | "weeks"
                | "day"
                | "days"
                | "month"
                | "months"
                | "tomorrow"
                | "today"
        )
}

fn is_preceding_blocker(token: &str) -> bool {
    matches!(
        token,
        "no" | "not"
            | "without"
            | "decline"
            | "declined"
            | "declines"
            | "refuse"
            | "refused"
            | "refuses"
            | "defer"
            | "deferred"
            | "avoid"
            | "cancel"
            | "cancelled"
            | "canceled"
    )
}

fn is_uncertain_blocker(token: &str) -> bool {
    matches!(
        token,
        "consider"
            | "considering"
            | "if"
            | "may"
            | "might"
            | "maybe"
            | "possible"
            | "possibly"
            | "unless"
    )
}

fn original_gap_has_hard_boundary(original: &str, start: usize, end: usize) -> bool {
    start < end
        && end <= original.len()
        && original[start..end]
            .chars()
            .any(|ch| matches!(ch, '.' | ';' | '\n' | '\r' | '!' | '?'))
}

fn dedupe_plan_matches(matches: &mut Vec<PlanEntityMatch>) {
    let mut seen = HashSet::new();
    matches.retain(|item| seen.insert((item.entity, item.span_start, item.span_end)));
    let snapshot = matches.clone();
    matches.retain(|item| {
        !snapshot.iter().any(|other| {
            other.entity == item.entity
                && other.span_start <= item.span_start
                && other.span_end >= item.span_end
                && (other.span_end - other.span_start) > (item.span_end - item.span_start)
        })
    });
}
