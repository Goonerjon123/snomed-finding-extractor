use crate::model::{AssertionStatus, SoapField};
use crate::normalization::normalize_clinical_text;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertionDecision {
    pub accepted: bool,
    pub assertion: AssertionStatus,
    pub rule_ids: Vec<String>,
    pub explanation: String,
}

#[derive(Debug, Clone)]
struct RuleHit {
    assertion: AssertionStatus,
    rule_id: &'static str,
    explanation: &'static str,
    priority: u8,
}

/// One word of the (normalised, shorthand-expanded) sentence containing the
/// match, with its original byte range and whether it belongs to another
/// terminology match in the same field.
#[derive(Debug, Clone)]
struct Token {
    text: String,
    orig_start: usize,
    orig_end: usize,
    sibling: bool,
    list_separator_before: bool,
    list_separator_after: bool,
}

/// Tokenised sentence with the match located inside it. `span_first` is the
/// index of the first token of the match; `span_after` the index just past
/// its last token.
#[derive(Debug)]
struct SentenceView {
    field: SoapField,
    tokens: Vec<Token>,
    span_first: usize,
    span_after: usize,
}

// ---------------------------------------------------------------------------
// Cue vocabularies. All phrases are written in normalised, shorthand-expanded
// form (lower case, single spaces).
// ---------------------------------------------------------------------------

const NEGATION_PHRASES: &[&[&str]] = &[
    &["no"],
    &["not"],
    &["denies"],
    &["denied"],
    &["deny"],
    &["denying"],
    &["without"],
    &["negative", "for"],
    &["free", "of"],
    &["absence", "of"],
    &["never", "had"],
    &["nil"],
];

const UNCERTAIN_PHRASES: &[&[&str]] = &[
    &["possible"],
    &["possibly"],
    &["probable"],
    &["suspected"],
    &["suspicion", "of"],
    &["query"],
    &["queried"],
    &["rule", "out"],
    &["r", "o"],
    &["differential"],
    &["consider"],
    &["considering"],
    &["concern", "about"],
    &["concerned", "about"],
    &["worried", "about"],
    &["worried", "re"],
    &["worry", "about"],
    &["worry", "re"],
];

const HISTORICAL_PHRASES: &[&[&str]] = &[
    &["past", "medical", "history"],
    &["past", "history"],
    &["history", "of"],
    &["previous"],
    &["previously"],
    &["resolved"],
    &["old"],
];

const NO_LONGER_PHRASE: &[&str] = &["no", "longer"];

const CONDITIONAL_PHRASES: &[&[&str]] = &[
    &["if"],
    &["unless"],
    &["should"],
    &["would"],
    &["could"],
    &["risk", "of"],
    &["in", "case", "of"],
];

const TRIGGER_FACTOR_PHRASES: &[&[&str]] = &[
    &["worse"],
    &["worsened", "by"],
    &["worse", "with"],
    &["worse", "after"],
    &["worse", "on"],
    &["triggered", "by"],
    &["brought", "on", "by"],
    &["set", "off", "by"],
];

const TRIGGER_GAP_ALLOW: &[&str] = &[
    "with", "c", "by", "on", "after", "when", "while", "during", "at", "and", "or", "plus",
    "sitting", "standing", "lying", "walking", "turning", "bending", "coughing", "stress", "foods",
    "food", "weather", "cold", "outdoors", "pollen", "high", "days", "exposure",
];

const PLANNED_PHRASES: &[&[&str]] = &[
    &["refer", "for"],
    &["referred", "for"],
    &["referral", "for"],
    &["test", "for"],
    &["testing", "for"],
    &["screen", "for"],
    &["screening", "for"],
    &["monitor", "for"],
    &["monitoring", "for"],
    &["arrange"],
    &["arranging"],
    &["plan", "to"],
    &["book"],
    &["order"],
];

const COMPLETED_ACTION_PHRASES: &[&[&str]] = &[
    &["started"],
    &["commenced"],
    &["given"],
    &["administered"],
    &["prescribed"],
    &["issued"],
    &["continued"],
    &["continuing"],
    &["restarted"],
    &["treated", "with"],
    &["switched", "to"],
];

const FAMILY_HISTORY_PHRASE: &[&str] = &["family", "history"];

const RELATIVES: &[&str] = &[
    "father",
    "mother",
    "mum",
    "mom",
    "dad",
    "brother",
    "brothers",
    "sister",
    "sisters",
    "sibling",
    "siblings",
    "son",
    "sons",
    "daughter",
    "daughters",
    "parent",
    "parents",
    "grandmother",
    "grandfather",
    "grandparent",
    "grandparents",
    "grandma",
    "grandad",
    "granddad",
    "aunt",
    "uncle",
    "cousin",
    "maternal",
    "paternal",
    "twin",
];

const NON_PATIENT: &[&str] = &[
    "wife",
    "husband",
    "partner",
    "carer",
    "friend",
    "colleague",
    "neighbour",
    "neighbor",
];

const CONTRAST: &[&str] = &[
    "but", "however", "although", "though", "whereas", "except", "apart", "besides",
];

/// Tokens a tight cue (negation/uncertainty/planned/historical) may scope
/// across on its way to the match. Anything else — affirmative verbs,
/// unrelated nouns — breaks the cue's scope, so "no improvement in cough"
/// and "no fever, has cough" leave the finding affirmed.
const TIGHT_GAP_ALLOW: &[&str] = &[
    "any",
    "evidence",
    "of",
    "signs",
    "sign",
    "symptoms",
    "symptom",
    "features",
    "feature",
    "complaints",
    "complaint",
    "further",
    "new",
    "significant",
    "obvious",
    "frank",
    "focal",
    "preceding",
    "type",
    "dvt",
    "acute",
    "associated",
    "current",
    "active",
    "ongoing",
    "known",
    "reported",
    "documented",
    "other",
    "more",
    "residual",
    "yet",
    "overt",
    "clinical",
    "visible",
    "audible",
    "palpable",
    "left",
    "right",
    "bilateral",
    "central",
    "peripheral",
    "exertional",
    "nocturnal",
    "productive",
    "dry",
    "severe",
    "mild",
    "moderate",
    "morning",
    "recent",
    "recurrent",
    "persistent",
    "intermittent",
    "chronic",
    "rest",
    "his",
    "her",
    "their",
    "the",
    "a",
    "an",
    "s",
    "or",
    "nor",
    "and",
    "such",
];

/// Body-site and laterality words that can sit between a negation cue and the
/// actual finding: "no limb weakness", "no spinal mass". Kept separate from
/// general gap words so it is clear this is anatomical qualification, not a
/// broad licence for arbitrary nouns.
const ANATOMICAL_GAP_ALLOW: &[&str] = &[
    "abdominal",
    "ankle",
    "arm",
    "arms",
    "back",
    "bladder",
    "bowel",
    "cervical",
    "chest",
    "cranial",
    "elbow",
    "ear",
    "ears",
    "eye",
    "eyes",
    "facial",
    "foot",
    "feet",
    "hand",
    "hands",
    "hip",
    "hips",
    "interdigital",
    "knee",
    "knees",
    "leg",
    "legs",
    "limb",
    "limbs",
    "lumbar",
    "neck",
    "pelvic",
    "sacral",
    "saddle",
    "shoulder",
    "shoulders",
    "spinal",
    "spine",
    "thoracic",
    "tip",
    "toe",
    "toes",
    "urinary",
    "wrist",
    "wrists",
];

const LIST_FRAGMENT_BLOCKERS: &[&str] = &[
    "feel",
    "feeling",
    "feels",
    "felt",
    "get",
    "gets",
    "getting",
    "got",
    "had",
    "has",
    "have",
    "having",
    "is",
    "present",
    "presents",
    "presenting",
    "reported",
    "reporting",
    "reports",
    "says",
    "with",
];

const DIRECT_NEGATION_MODIFIER_BLOCKERS: &[&str] = &[
    "about",
    "after",
    "before",
    "by",
    "despite",
    "due",
    "during",
    "for",
    "from",
    "in",
    "into",
    "near",
    "on",
    "onto",
    "over",
    "re",
    "regarding",
    "since",
    "to",
    "with",
    "within",
    "change",
    "changes",
    "chance",
    "concern",
    "concerns",
    "decrease",
    "decreased",
    "improve",
    "improved",
    "improving",
    "improvement",
    "increase",
    "increased",
    "reduced",
    "reduction",
    "relief",
    "risk",
    "worse",
    "worsen",
    "worsened",
    "worsening",
];

/// Connectors a family/non-patient experiencer may scope across before the
/// match. Possessive verbs stay in scope ("mother has diabetes"); reporting
/// verbs break it ("mother says he has a cough" is about the patient).
const EXPERIENCER_GAP_ALLOW: &[&str] = &[
    "has",
    "had",
    "have",
    "having",
    "also",
    "both",
    "all",
    "died",
    "dies",
    "dying",
    "death",
    "deceased",
    "suffered",
    "suffers",
    "suffering",
    "developed",
    "diagnosed",
    "with",
    "of",
    "known",
    "and",
    "or",
    "a",
    "an",
    "the",
    "his",
    "her",
    "their",
    "my",
    "aged",
    "age",
    "at",
    "in",
    "from",
    "late",
    "young",
    "early",
    "recently",
    "previous",
    "previously",
    "history",
    "passed",
    "away",
    "s",
    "side",
    "family",
];

/// Connectors allowed between the match and a following experiencer:
/// "diabetes in his mother".
const EXPERIENCER_AFTER_ALLOW: &[&str] = &[
    "in", "for", "of", "on", "his", "her", "their", "the", "my", "s",
];

/// Verb-tolerant gap for "no longer": "no longer has chest pain".
const NO_LONGER_GAP_ALLOW: &[&str] = &[
    "has",
    "have",
    "having",
    "had",
    "any",
    "the",
    "his",
    "her",
    "their",
    "experiencing",
    "getting",
    "gets",
    "suffering",
    "suffers",
    "complaining",
    "reporting",
    "troubled",
    "by",
];

/// Tokens after the match that keep a trailing "resolved" in scope:
/// "chest pain has now resolved".
const RESOLVED_AFTER_ALLOW: &[&str] = &[
    "has",
    "have",
    "had",
    "now",
    "since",
    "completely",
    "fully",
    "essentially",
    "largely",
    "mostly",
    "all",
];

const RESOLVED_POSTFIX: &[&str] = &["resolved", "settled"];

/// Tokens that void a completed-action override because the action was
/// informational rather than therapeutic: "given advice re sepsis".
const COMPLETED_ACTION_BLOCKERS: &[&str] = &[
    "advice",
    "advise",
    "advised",
    "leaflet",
    "leaflets",
    "information",
    "education",
    "counselling",
    "counseling",
    "signposted",
];

const DURATION_WORDS: &[&str] = &[
    "day",
    "days",
    "week",
    "weeks",
    "wk",
    "wks",
    "month",
    "months",
    "mth",
    "mths",
    "year",
    "years",
    "yr",
    "yrs",
    "long",
    "lifelong",
    "longstanding",
];

// ---------------------------------------------------------------------------

/// Classifies one terminology match. `sibling_spans` are the original-text
/// spans of the other matches found in the same field; tokens they cover may
/// carry a cue across a coordination ("no cough or wheeze") without breaking
/// its scope.
pub fn classify_assertion(
    field: SoapField,
    field_text: &str,
    span_start: usize,
    span_end: usize,
    sibling_spans: &[(usize, usize)],
) -> AssertionDecision {
    let (sentence_start, sentence_end) = sentence_bounds(field_text, span_start, span_end);
    let sentence = &field_text[sentence_start..sentence_end];
    let rel_span = (span_start - sentence_start, span_end - sentence_start);
    let rel_siblings = sibling_spans
        .iter()
        .filter(|(start, end)| *end > sentence_start && *start < sentence_end)
        .map(|(start, end)| {
            (
                start.saturating_sub(sentence_start),
                (end - sentence_start).min(sentence.len()),
            )
        })
        .collect::<Vec<_>>();

    let view = sentence_view(field, sentence, rel_span, &rel_siblings);
    let query_prefix = query_prefix_applies(&field_text[sentence_start..span_start]);

    let mut hits = Vec::new();

    if family_history_frame(&view) || experiencer_applies(&view, RELATIVES) {
        hits.push(RuleHit {
            assertion: AssertionStatus::FamilyHistory,
            rule_id: "CTX_FAMILY_HISTORY",
            explanation: "the mention is bound to family-history or relative context",
            priority: 10,
        });
    }

    if experiencer_applies(&view, NON_PATIENT) {
        hits.push(RuleHit {
            assertion: AssertionStatus::NonPatient,
            rule_id: "CTX_NON_PATIENT_EXPERIENCER",
            explanation: "the mention is bound to someone other than the patient",
            priority: 11,
        });
    }

    if negation_cue_applies(&view) {
        hits.push(RuleHit {
            assertion: AssertionStatus::Negated,
            rule_id: "CTX_NEGATED_PRECEDING",
            explanation: "a negation cue scopes directly over the mention",
            priority: 20,
        });
    }

    if tight_cue_applies(&view, UNCERTAIN_PHRASES, TIGHT_GAP_ALLOW) || query_prefix {
        hits.push(RuleHit {
            assertion: AssertionStatus::Uncertain,
            rule_id: "CTX_UNCERTAIN_OR_QUERY",
            explanation: "an uncertainty or query cue scopes directly over the mention",
            priority: 30,
        });
    }

    if historical_applies(&view) {
        hits.push(RuleHit {
            assertion: AssertionStatus::HistoricalOrResolved,
            rule_id: "CTX_HISTORICAL_OR_RESOLVED",
            explanation: "the mention is framed as historical, previous, or resolved",
            priority: 50,
        });
    }

    if frame_cue_applies(&view, CONDITIONAL_PHRASES) {
        hits.push(RuleHit {
            assertion: AssertionStatus::Conditional,
            rule_id: "CTX_CONDITIONAL_OR_HYPOTHETICAL",
            explanation: "the mention is conditional or hypothetical",
            priority: 60,
        });
    }

    if trigger_factor_applies(&view) {
        hits.push(RuleHit {
            assertion: AssertionStatus::Ambiguous,
            rule_id: "CTX_TRIGGER_OR_AGGRAVATING_FACTOR",
            explanation:
                "the mention is framed as a trigger or aggravating factor rather than a symptom",
            priority: 65,
        });
    }

    if tight_cue_applies(&view, PLANNED_PHRASES, TIGHT_GAP_ALLOW) {
        hits.push(RuleHit {
            assertion: AssertionStatus::Planned,
            rule_id: "CTX_PLANNED_ACTION",
            explanation:
                "the mention is the target of a planned action rather than an asserted concept",
            priority: 41,
        });
    }

    let plan_completed_override = field == SoapField::Plan && completed_action_override(&view);
    if field == SoapField::Plan && !plan_completed_override {
        hits.push(RuleHit {
            assertion: AssertionStatus::Planned,
            rule_id: "PLAN_FIELD_REVIEW_ONLY",
            explanation:
                "plan field mentions are review-only unless a completed action asserts them",
            priority: 40,
        });
    }

    if hits.is_empty() {
        let mut rule_ids = vec!["ASSERT_AFFIRMED_PATIENT_FINDING".to_string()];
        let explanation = if plan_completed_override {
            rule_ids.push("PLAN_COMPLETED_ACTION".to_string());
            format!(
                "Accepted: a completed action in the {} field asserts the mention.",
                field.as_str()
            )
        } else {
            format!(
                "Accepted as an affirmed patient finding in the {} field; no suppression rule fired.",
                field.as_str()
            )
        };
        return AssertionDecision {
            accepted: true,
            assertion: AssertionStatus::Affirmed,
            rule_ids,
            explanation,
        };
    }

    hits.sort_by_key(|hit| hit.priority);
    hits.dedup_by_key(|hit| hit.rule_id);
    let primary = hits[0].assertion;
    let rule_ids = hits
        .iter()
        .map(|hit| hit.rule_id.to_string())
        .collect::<Vec<_>>();
    let explanations = hits
        .iter()
        .map(|hit| hit.explanation)
        .collect::<Vec<_>>()
        .join("; ");

    AssertionDecision {
        accepted: false,
        assertion: primary,
        rule_ids,
        explanation: format!("Suppressed: {explanations}."),
    }
}

// ---------------------------------------------------------------------------
// Sentence/token machinery
// ---------------------------------------------------------------------------

fn sentence_bounds(text: &str, span_start: usize, span_end: usize) -> (usize, usize) {
    let start = text[..span_start]
        .rfind(['.', '!', ';', '\n', '\r'])
        .map(|idx| idx + 1)
        .unwrap_or(0);
    let end = text[span_end..]
        .find(['.', '!', '?', ';', '\n', '\r'])
        .map(|idx| span_end + idx)
        .unwrap_or(text.len());
    (start, end)
}

fn query_prefix_applies(prefix: &str) -> bool {
    let prefix = prefix.trim_end();
    let Some(before_query) = prefix.strip_suffix('?') else {
        return false;
    };
    let before_query = before_query.trim_end();
    before_query.is_empty() || before_query.ends_with(':')
}

fn sentence_view(
    field: SoapField,
    sentence: &str,
    rel_span: (usize, usize),
    rel_siblings: &[(usize, usize)],
) -> SentenceView {
    let normalized = normalize_clinical_text(sentence, field);
    let mut tokens = Vec::new();
    let mut cursor = 0;
    let text = normalized.text.as_str();

    while cursor < text.len() {
        let remaining = &text[cursor..];
        let token_start = match remaining.find(|ch| ch != ' ') {
            Some(offset) => cursor + offset,
            None => break,
        };
        let token_end = text[token_start..]
            .find(' ')
            .map(|offset| token_start + offset)
            .unwrap_or(text.len());

        if let Some((orig_start, orig_end)) = normalized.original_range(token_start, token_end) {
            let overlaps = |range: (usize, usize)| orig_start < range.1 && orig_end > range.0;
            tokens.push(Token {
                text: text[token_start..token_end].to_string(),
                orig_start,
                orig_end,
                sibling: rel_siblings.iter().any(|range| overlaps(*range)),
                list_separator_before: false,
                list_separator_after: false,
            });
        }

        cursor = token_end;
    }

    for index in 0..tokens.len().saturating_sub(1) {
        let has_separator = has_list_separator_between(
            sentence,
            tokens[index].orig_end,
            tokens[index + 1].orig_start,
        );
        tokens[index].list_separator_after = has_separator;
        tokens[index + 1].list_separator_before = has_separator;
    }

    let span_first = tokens
        .iter()
        .position(|token| token.orig_end > rel_span.0)
        .unwrap_or(tokens.len());
    let span_after = tokens
        .iter()
        .position(|token| token.orig_start >= rel_span.1)
        .unwrap_or(tokens.len());

    SentenceView {
        field,
        tokens,
        span_first,
        span_after: span_after.max(span_first),
    }
}

/// Span of the last phrase from `phrases` finishing at or before `limit`.
fn last_phrase_span(tokens: &[Token], limit: usize, phrases: &[&[&str]]) -> Option<(usize, usize)> {
    let mut last = None;
    for phrase in phrases {
        if phrase.is_empty() || phrase.len() > limit {
            continue;
        }
        for start in 0..=(limit - phrase.len()) {
            let matched = phrase
                .iter()
                .enumerate()
                .all(|(offset, word)| tokens[start + offset].text == *word);
            if matched {
                let end = start + phrase.len();
                if last
                    .map(|(_, previous_end)| end > previous_end)
                    .unwrap_or(true)
                {
                    last = Some((start, end));
                }
            }
        }
    }
    last
}

/// End index (exclusive) of the last phrase from `phrases` finishing at or
/// before `limit`.
fn last_phrase_end(tokens: &[Token], limit: usize, phrases: &[&[&str]]) -> Option<usize> {
    last_phrase_span(tokens, limit, phrases).map(|(_, end)| end)
}

fn is_contrast(token: &Token) -> bool {
    CONTRAST.contains(&token.text.as_str())
}

fn has_digit(token: &Token) -> bool {
    token.text.chars().any(|ch| ch.is_ascii_digit())
}

fn gap_is_clear(tokens: &[Token], gap: std::ops::Range<usize>, allow: &[&str]) -> bool {
    tokens[gap].iter().all(|token| {
        !is_contrast(token)
            && (token.sibling || has_digit(token) || allow.contains(&token.text.as_str()))
    })
}

fn tight_gap_is_clear(
    tokens: &[Token],
    gap: std::ops::Range<usize>,
    allow: &[&str],
    match_start: usize,
) -> bool {
    gap.clone()
        .all(|index| tight_gap_token_is_clear(tokens, index, allow, match_start))
}

fn tight_gap_token_is_clear(
    tokens: &[Token],
    index: usize,
    allow: &[&str],
    match_start: usize,
) -> bool {
    let token = &tokens[index];
    if starts_affirmed_finding_after_list_separator(token) {
        return false;
    }
    !is_contrast(token)
        && (token.sibling
            || has_digit(token)
            || allow.contains(&token.text.as_str())
            || ANATOMICAL_GAP_ALLOW.contains(&token.text.as_str())
            || is_negated_list_fragment(tokens, index, match_start))
}

fn starts_affirmed_finding_after_list_separator(token: &Token) -> bool {
    token.list_separator_before
        && matches!(
            token.text.as_str(),
            "mild"
                | "mildly"
                | "moderate"
                | "moderately"
                | "severe"
                | "severely"
                | "small"
                | "large"
                | "significant"
        )
}

fn is_negated_list_fragment(tokens: &[Token], index: usize, match_start: usize) -> bool {
    if index >= match_start {
        return false;
    }
    let token = &tokens[index];
    if token.text.chars().any(|ch| ch.is_ascii_digit()) || token.text.len() > 24 {
        return false;
    }
    if LIST_FRAGMENT_BLOCKERS.contains(&token.text.as_str()) {
        return false;
    }
    if token.text.ends_with("ing") || token.text.ends_with("ed") {
        return false;
    }

    token.list_separator_before
        || token.list_separator_after
        || tokens
            .get(index + 1)
            .map(|next| matches!(next.text.as_str(), "and" | "or" | "nor"))
            .unwrap_or(false)
}

fn has_list_separator_between(sentence: &str, start: usize, end: usize) -> bool {
    if start >= end || end > sentence.len() {
        return false;
    }
    sentence[start..end]
        .chars()
        .any(|ch| matches!(ch, '/' | ',' | '+'))
}

/// A tight cue scopes over the match only when every token between the cue
/// and the match is an allowed descriptor, a digit, or part of another
/// terminology match joined by coordination.
fn tight_cue_applies(view: &SentenceView, phrases: &[&[&str]], allow: &[&str]) -> bool {
    let Some(cue_end) = last_phrase_end(&view.tokens, view.span_first, phrases) else {
        return false;
    };
    tight_gap_is_clear(
        &view.tokens,
        cue_end..view.span_first,
        allow,
        view.span_first,
    )
}

fn negation_cue_applies(view: &SentenceView) -> bool {
    let Some((_, cue_end)) = last_phrase_span(&view.tokens, view.span_first, NEGATION_PHRASES)
    else {
        return false;
    };

    let gap = cue_end..view.span_first;
    if view.field == SoapField::Objective
        && objective_exam_result_starts_at_match(view)
        && gap.clone().any(|index| {
            has_digit(&view.tokens[index])
                || view.tokens[index].list_separator_before
                || view.tokens[index].list_separator_after
        })
    {
        return false;
    }

    tight_gap_is_clear(&view.tokens, gap.clone(), TIGHT_GAP_ALLOW, view.span_first)
        || direct_negation_modifier_gap_is_clear(&view.tokens, gap, view.span_first)
}

fn objective_exam_result_starts_at_match(view: &SentenceView) -> bool {
    let mut index = view.span_after;
    let limit = (view.span_after + 4).min(view.tokens.len());
    while index < limit {
        let token = view.tokens[index].text.as_str();
        if objective_exam_result_status_token(token) || has_digit(&view.tokens[index]) {
            return true;
        }
        if !objective_exam_shared_feature_token(token) && !objective_exam_result_bridge_token(token)
        {
            return false;
        }
        index += 1;
    }

    false
}

fn objective_exam_result_status_token(token: &str) -> bool {
    matches!(
        token,
        "normal"
            | "clear"
            | "intact"
            | "symmetrical"
            | "symmetric"
            | "full"
            | "present"
            | "palpable"
            | "pulsatile"
            | "regular"
            | "equal"
    )
}

fn objective_exam_shared_feature_token(token: &str) -> bool {
    matches!(
        token,
        "coordination"
            | "gait"
            | "power"
            | "reflex"
            | "reflexes"
            | "range"
            | "movement"
            | "rom"
            | "sensation"
            | "tone"
    )
}

fn objective_exam_result_bridge_token(token: &str) -> bool {
    matches!(token, "and" | "or" | "plus" | "is" | "are")
}

fn direct_negation_modifier_gap_is_clear(
    tokens: &[Token],
    gap: std::ops::Range<usize>,
    match_start: usize,
) -> bool {
    let gap_len = gap.end.saturating_sub(gap.start);
    if gap_len == 0 || gap_len > 4 {
        return false;
    }

    let mut inferred_modifiers = 0usize;
    for index in gap {
        let token = &tokens[index];
        if starts_affirmed_finding_after_list_separator(token) || is_contrast(token) {
            return false;
        }
        if tight_gap_token_is_clear(tokens, index, TIGHT_GAP_ALLOW, match_start) {
            continue;
        }
        if safe_direct_negation_modifier(token) {
            inferred_modifiers += 1;
            if inferred_modifiers <= 2 {
                continue;
            }
        }
        return false;
    }

    true
}

fn safe_direct_negation_modifier(token: &Token) -> bool {
    let text = token.text.as_str();
    text.len() >= 2
        && text.len() <= 18
        && text.chars().all(|ch| ch.is_ascii_alphabetic())
        && !text.ends_with("ed")
        && !text.ends_with("ing")
        && !LIST_FRAGMENT_BLOCKERS.contains(&text)
        && !DIRECT_NEGATION_MODIFIER_BLOCKERS.contains(&text)
}

/// A frame cue (conditional, family-history heading) applies to the rest of
/// its sentence unless a contrast word intervenes.
fn frame_cue_applies(view: &SentenceView, phrases: &[&[&str]]) -> bool {
    let Some(cue_end) = last_phrase_end(&view.tokens, view.span_first, phrases) else {
        return false;
    };
    !view.tokens[cue_end..view.span_first]
        .iter()
        .any(is_contrast)
}

fn trigger_factor_applies(view: &SentenceView) -> bool {
    let Some(cue_end) = last_phrase_end(&view.tokens, view.span_first, TRIGGER_FACTOR_PHRASES)
    else {
        return false;
    };

    gap_is_clear(&view.tokens, cue_end..view.span_first, TRIGGER_GAP_ALLOW)
}

fn family_history_frame(view: &SentenceView) -> bool {
    frame_cue_applies(view, &[FAMILY_HISTORY_PHRASE])
}

/// Relative/non-patient experiencers bind to the match through possessive
/// connectors, before ("mother has diabetes") or after ("diabetes in his
/// mother") the mention.
fn experiencer_applies(view: &SentenceView, experiencers: &[&str]) -> bool {
    let before = view.tokens[..view.span_first]
        .iter()
        .rposition(|token| experiencers.contains(&token.text.as_str()));
    if let Some(idx) = before {
        if gap_is_clear(
            &view.tokens,
            idx + 1..view.span_first,
            EXPERIENCER_GAP_ALLOW,
        ) {
            return true;
        }
    }

    let after = view.tokens[view.span_after..]
        .iter()
        .position(|token| experiencers.contains(&token.text.as_str()))
        .map(|offset| view.span_after + offset);
    if let Some(idx) = after {
        if gap_is_clear(&view.tokens, view.span_after..idx, EXPERIENCER_AFTER_ALLOW) {
            return true;
        }
    }

    false
}

fn historical_applies(view: &SentenceView) -> bool {
    if let Some(cue_end) = last_phrase_end(&view.tokens, view.span_first, HISTORICAL_PHRASES) {
        let suppressed_by_duration = is_duration_qualified_history(view, cue_end);
        if !suppressed_by_duration
            && gap_is_clear(&view.tokens, cue_end..view.span_first, TIGHT_GAP_ALLOW)
        {
            return true;
        }
    }

    // "no longer has chest pain"
    if let Some(cue_end) = last_phrase_end(&view.tokens, view.span_first, &[NO_LONGER_PHRASE]) {
        if gap_is_clear(&view.tokens, cue_end..view.span_first, NO_LONGER_GAP_ALLOW) {
            return true;
        }
    }

    // "chest pain has now resolved"
    let postfix = view.tokens[view.span_after..]
        .iter()
        .position(|token| RESOLVED_POSTFIX.contains(&token.text.as_str()))
        .map(|offset| view.span_after + offset);
    if let Some(idx) = postfix {
        if gap_is_clear(&view.tokens, view.span_after..idx, RESOLVED_AFTER_ALLOW) {
            return true;
        }
    }

    false
}

/// "3 day history of cough" and "2/52 history of chest pain" are presenting
/// complaints, not past history: a duration immediately before a bare
/// "history of" disarms the historical cue.
fn is_duration_qualified_history(view: &SentenceView, cue_end: usize) -> bool {
    let phrase_len = FAMILY_HISTORY_PHRASE.len(); // "history of" is also two tokens
    if cue_end < phrase_len
        || view.tokens[cue_end - 2].text != "history"
        || view.tokens[cue_end - 1].text != "of"
    {
        return false;
    }
    if cue_end >= 3 && view.tokens[cue_end - 3].text == "past" {
        return false;
    }
    view.tokens
        .get(cue_end.wrapping_sub(3))
        .map(|token| has_digit(token) || DURATION_WORDS.contains(&token.text.as_str()))
        .unwrap_or(false)
}

/// In the Plan field a completed therapeutic action asserts its target:
/// "Started amoxicillin for LRTI". The override loses to any nearer planned,
/// conditional, uncertainty, or negation cue, and to advice-style actions.
fn completed_action_override(view: &SentenceView) -> bool {
    let Some(completed_end) =
        last_phrase_end(&view.tokens, view.span_first, COMPLETED_ACTION_PHRASES)
    else {
        return false;
    };

    let competing = [
        last_phrase_end(&view.tokens, view.span_first, NEGATION_PHRASES),
        last_phrase_end(&view.tokens, view.span_first, UNCERTAIN_PHRASES),
        last_phrase_end(&view.tokens, view.span_first, PLANNED_PHRASES),
        last_phrase_end(&view.tokens, view.span_first, CONDITIONAL_PHRASES),
    ];
    if competing
        .iter()
        .flatten()
        .any(|other_end| *other_end > completed_end)
    {
        return false;
    }

    view.tokens[completed_end..view.span_first]
        .iter()
        .all(|token| {
            !is_contrast(token) && !COMPLETED_ACTION_BLOCKERS.contains(&token.text.as_str())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(field: SoapField, text: &str, target: &str) -> AssertionDecision {
        classify_with_siblings(field, text, target, &[])
    }

    fn classify_with_siblings(
        field: SoapField,
        text: &str,
        target: &str,
        sibling_texts: &[&str],
    ) -> AssertionDecision {
        let start = text.find(target).expect("target not in text");
        let siblings = sibling_texts
            .iter()
            .map(|sibling| {
                let idx = text.find(sibling).expect("sibling not in text");
                (idx, idx + sibling.len())
            })
            .collect::<Vec<_>>();
        classify_assertion(field, text, start, start + target.len(), &siblings)
    }

    // --- negation scope ---------------------------------------------------

    #[test]
    fn suppresses_directly_negated_mentions() {
        let decision = classify(SoapField::History, "No chest pain today", "chest pain");
        assert!(!decision.accepted);
        assert_eq!(decision.assertion, AssertionStatus::Negated);
    }

    #[test]
    fn negation_scopes_across_descriptors_and_pertinent_qualifiers() {
        for (text, target) in [
            ("No evidence of chest pain", "chest pain"),
            ("Denies any chest pain", "chest pain"),
            ("No new chest pain since", "chest pain"),
            ("No morning headache", "headache"),
            ("No rest pain", "pain"),
            ("No shoulder-tip pain", "pain"),
        ] {
            let decision = classify(SoapField::History, text, target);
            assert!(!decision.accepted, "expected suppression for {text}");
            assert_eq!(decision.assertion, AssertionStatus::Negated);
        }
    }

    #[test]
    fn affirmative_verb_after_negation_ends_its_scope() {
        let decision = classify(SoapField::History, "No fever, has cough", "cough");
        assert!(decision.accepted);
    }

    #[test]
    fn negation_bound_to_another_noun_does_not_suppress() {
        let decision = classify(SoapField::History, "No improvement in cough", "cough");
        assert!(decision.accepted);
    }

    #[test]
    fn distant_negation_does_not_leak_onto_the_mention() {
        let decision = classify(
            SoapField::History,
            "Not sure why he gets chest pain",
            "chest pain",
        );
        assert!(decision.accepted);
    }

    #[test]
    fn contrast_ends_negation_scope() {
        let decision = classify(
            SoapField::History,
            "Denies chest pain but has cough",
            "cough",
        );
        assert!(decision.accepted);
    }

    #[test]
    fn negation_propagates_across_coordinated_sibling_matches() {
        let decision = classify_with_siblings(
            SoapField::History,
            "No cough or wheeze",
            "wheeze",
            &["cough"],
        );
        assert!(!decision.accepted);
        assert_eq!(decision.assertion, AssertionStatus::Negated);
    }

    #[test]
    fn negation_scopes_over_punctuated_lists_with_unknown_items() {
        for text in [
            "Denies ABC / target marker / future plans",
            "Denies ABC, target marker, future plans",
            "Denies ABC or target marker",
        ] {
            let decision = classify(SoapField::History, text, "target marker");
            assert!(!decision.accepted, "expected suppression for {text}");
            assert_eq!(decision.assertion, AssertionStatus::Negated);
        }

        let decision = classify_with_siblings(
            SoapField::History,
            "No first marker / alpha beta / target marker",
            "target marker",
            &["first marker"],
        );
        assert!(!decision.accepted);
        assert_eq!(decision.assertion, AssertionStatus::Negated);
    }

    #[test]
    fn negation_scopes_across_anatomical_qualifiers() {
        for (text, target, siblings) in [
            ("No limb weakness", "weakness", vec![]),
            ("No limb weakness/numbness", "numbness", vec!["weakness"]),
            ("No spinal step/mass", "mass", vec!["step"]),
            ("No ear pain", "pain", vec![]),
            ("No DVT-type calf pain", "calf pain", vec![]),
            ("No interdigital maceration", "maceration", vec![]),
        ] {
            let decision = classify_with_siblings(SoapField::History, text, target, &siblings);
            assert!(!decision.accepted, "expected suppression for {text}");
            assert_eq!(decision.assertion, AssertionStatus::Negated);
        }
    }

    #[test]
    fn negation_scopes_over_directly_qualified_symptoms() {
        for (text, target) in [
            ("No early-morning vomiting", "vomiting"),
            ("No jaw claudication", "claudication"),
            ("No right upper quadrant pain", "pain"),
            ("Denies scalp tenderness", "tenderness"),
        ] {
            let decision = classify(SoapField::History, text, target);
            assert!(!decision.accepted, "expected suppression for {text}");
            assert_eq!(decision.assertion, AssertionStatus::Negated);
        }
    }

    #[test]
    fn repeated_negation_in_red_flag_lists_scopes_to_each_qualified_symptom() {
        let text = "No visual loss, no weakness/numbness, no fever/neck stiffness, not worse lying/coughing, no early-morning vomiting, no jaw claudication.";

        for target in ["vomiting", "claudication"] {
            let decision = classify(SoapField::History, text, target);
            assert!(!decision.accepted, "expected suppression for {target}");
            assert_eq!(decision.assertion, AssertionStatus::Negated);
        }

        let cough = classify(SoapField::History, text, "coughing");
        assert!(!cough.accepted);
        assert_eq!(cough.assertion, AssertionStatus::Ambiguous);
    }

    #[test]
    fn comma_after_negated_list_can_start_affirmed_finding() {
        let decision = classify(
            SoapField::Objective,
            "Fundi - no haemorrhages/exudates, mild AV nipping",
            "AV nipping",
        );
        assert!(decision.accepted);
    }

    #[test]
    fn objective_parenthetical_negation_does_not_leak_to_normal_exam_results() {
        let text =
            "Fundi normal (no papilloedema), power 5/5, reflexes symmetrical, coordination + gait normal.";

        for target in ["reflexes", "coordination", "gait"] {
            let decision = classify(SoapField::Objective, text, target);
            assert!(decision.accepted, "expected acceptance for {target}");
        }
    }

    #[test]
    fn laterality_body_site_prefix_does_not_make_finding_uncertain() {
        let decision = classify(
            SoapField::Objective,
            "L lower leg - erythema, warmth and swelling",
            "erythema",
        );
        assert!(decision.accepted);
    }

    #[test]
    fn objective_laterality_prefix_stays_affirmed_with_sibling_match() {
        let decision = classify_with_siblings(
            SoapField::Objective,
            "L lower leg - erythema, warmth + swelling over shin",
            "erythema",
            &["warmth"],
        );
        assert!(decision.accepted);
    }

    #[test]
    fn objective_full_note_laterality_prefix_stays_affirmed() {
        let text = "O/E: T 38.1, HR 96. L lower leg — erythema, warmth + swelling over shin, ~12x8cm, demarcated + outlined in pen.";
        let decision = classify_with_siblings(SoapField::Objective, text, "erythema", &["warmth"]);
        assert!(decision.accepted);
    }

    #[test]
    fn not_yet_scopes_as_negation() {
        let decision = classify(SoapField::History, "Not yet target marker", "target marker");
        assert!(!decision.accepted);
        assert_eq!(decision.assertion, AssertionStatus::Negated);
    }

    // --- experiencer ------------------------------------------------------

    #[test]
    fn relative_with_possessive_verb_is_family_history() {
        let decision = classify(SoapField::History, "Mother has diabetes", "diabetes");
        assert!(!decision.accepted);
        assert_eq!(decision.assertion, AssertionStatus::FamilyHistory);
    }

    #[test]
    fn relative_after_the_mention_is_family_history() {
        let decision = classify(SoapField::History, "Diabetes in his mother", "Diabetes");
        assert!(!decision.accepted);
        assert_eq!(decision.assertion, AssertionStatus::FamilyHistory);
    }

    #[test]
    fn social_mention_of_relative_does_not_suppress() {
        let decision = classify(
            SoapField::History,
            "Lives with his mother and reports chest pain",
            "chest pain",
        );
        assert!(decision.accepted);
    }

    #[test]
    fn family_history_heading_frames_the_sentence() {
        let decision = classify(
            SoapField::History,
            "Family history of bowel cancer and diabetes",
            "diabetes",
        );
        assert!(!decision.accepted);
        assert_eq!(decision.assertion, AssertionStatus::FamilyHistory);
    }

    #[test]
    fn shorthand_fh_expands_into_a_family_history_frame() {
        let decision = classify(SoapField::History, "FHx diabetes", "diabetes");
        assert!(!decision.accepted);
        assert_eq!(decision.assertion, AssertionStatus::FamilyHistory);
    }

    #[test]
    fn non_patient_experiencer_is_suppressed() {
        let decision = classify(SoapField::History, "Wife has a cough", "cough");
        assert!(!decision.accepted);
        assert_eq!(decision.assertion, AssertionStatus::NonPatient);
    }

    #[test]
    fn reporting_relative_keeps_the_patients_finding() {
        let decision = classify(
            SoapField::History,
            "Mother says he has had a cough",
            "cough",
        );
        assert!(decision.accepted);
    }

    // --- historical -------------------------------------------------------

    #[test]
    fn past_history_is_suppressed() {
        let decision = classify(SoapField::History, "Past history of asthma", "asthma");
        assert!(!decision.accepted);
        assert_eq!(decision.assertion, AssertionStatus::HistoricalOrResolved);
    }

    #[test]
    fn duration_qualified_history_is_a_presenting_complaint() {
        for text in [
            "3 day history of cough",
            "2/52 history of cough",
            "Long history of cough",
        ] {
            let decision = classify(SoapField::History, text, "cough");
            assert!(decision.accepted, "expected acceptance for {text}");
        }
    }

    #[test]
    fn no_longer_and_trailing_resolved_are_historical() {
        for (text, target) in [
            ("No longer has chest pain", "chest pain"),
            ("Chest pain has now resolved", "Chest pain"),
        ] {
            let decision = classify(SoapField::History, text, target);
            assert!(!decision.accepted, "expected suppression for {text}");
            assert_eq!(decision.assertion, AssertionStatus::HistoricalOrResolved);
        }
    }

    // --- shorthand-driven cues ---------------------------------------------

    #[test]
    fn h_slash_o_expands_to_history_of() {
        let decision = classify(SoapField::History, "h/o depression", "depression");
        assert!(!decision.accepted);
        assert_eq!(decision.assertion, AssertionStatus::HistoricalOrResolved);
    }

    // --- plan field ---------------------------------------------------------

    #[test]
    fn plan_mentions_stay_review_only_by_default() {
        let decision = classify(SoapField::Plan, "Screen for depression", "depression");
        assert!(!decision.accepted);
        assert_eq!(decision.assertion, AssertionStatus::Planned);
    }

    #[test]
    fn completed_action_asserts_its_target_in_plan() {
        let decision = classify(
            SoapField::Plan,
            "Started sertraline for depression",
            "depression",
        );
        assert!(decision.accepted);
        assert!(decision
            .rule_ids
            .iter()
            .any(|id| id == "PLAN_COMPLETED_ACTION"));
    }

    #[test]
    fn nearer_planned_cue_beats_completed_action() {
        let decision = classify(
            SoapField::Plan,
            "Started celecoxib, monitor for dyspepsia",
            "dyspepsia",
        );
        assert!(!decision.accepted);
        assert_eq!(decision.assertion, AssertionStatus::Planned);
    }

    #[test]
    fn advice_giving_is_not_a_completed_therapeutic_action() {
        let decision = classify(SoapField::Plan, "Given advice about asthma", "asthma");
        assert!(!decision.accepted);
        assert_eq!(decision.assertion, AssertionStatus::Planned);
    }

    // --- uncertainty and conditional ----------------------------------------

    #[test]
    fn query_prefix_is_uncertain() {
        let decision = classify(SoapField::Assessment, "?pneumonia", "pneumonia");
        assert!(!decision.accepted);
        assert_eq!(decision.assertion, AssertionStatus::Uncertain);
    }

    #[test]
    fn query_prefix_after_heading_is_uncertain() {
        let decision = classify(
            SoapField::Assessment,
            "Impression: ? pneumonia",
            "pneumonia",
        );
        assert!(!decision.accepted);
        assert_eq!(decision.assertion, AssertionStatus::Uncertain);
    }

    #[test]
    fn question_mark_separator_after_site_label_is_not_uncertain() {
        let decision = classify(
            SoapField::Objective,
            "ENT ? throat injected",
            "throat injected",
        );
        assert!(decision.accepted);
    }

    #[test]
    fn concern_wording_marks_following_concept_uncertain() {
        for text in [
            "Worried about target condition",
            "Worried re: target condition",
        ] {
            let decision = classify(SoapField::History, text, "target condition");
            assert!(!decision.accepted, "expected uncertainty for {text}");
            assert_eq!(decision.assertion, AssertionStatus::Uncertain);
        }
    }

    #[test]
    fn conditional_frame_holds_across_the_clause() {
        let decision = classify(
            SoapField::Plan,
            "If she develops chest pain call 999",
            "chest pain",
        );
        assert!(!decision.accepted);
        assert!(decision
            .rule_ids
            .iter()
            .any(|id| id == "CTX_CONDITIONAL_OR_HYPOTHETICAL"));
    }

    #[test]
    fn temporal_symptom_description_is_not_conditional() {
        let decision = classify(
            SoapField::History,
            "Gets dizzy when standing quickly",
            "dizzy",
        );
        assert!(decision.accepted);
    }

    #[test]
    fn suppresses_trigger_or_aggravating_factor_mentions() {
        for (text, target) in [
            ("Pain worse sitting and coughing", "coughing"),
            ("Rash worse c stress", "stress"),
            ("Symptoms triggered by cold weather", "cold"),
        ] {
            let decision = classify(SoapField::History, text, target);
            assert!(!decision.accepted, "expected suppression for {text}");
            assert_eq!(decision.assertion, AssertionStatus::Ambiguous);
            assert!(decision
                .rule_ids
                .iter()
                .any(|id| id == "CTX_TRIGGER_OR_AGGRAVATING_FACTOR"));
        }
    }

    #[test]
    fn negation_scopes_across_preceding_qualifier_and_sibling() {
        let decision = classify_with_siblings(
            SoapField::History,
            "No preceding chest pain/palpitations",
            "palpitations",
            &["chest pain"],
        );
        assert!(!decision.accepted);
        assert_eq!(decision.assertion, AssertionStatus::Negated);
    }
}
