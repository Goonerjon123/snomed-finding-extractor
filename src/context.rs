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
    list_separator_after: bool,
}

/// Tokenised sentence with the match located inside it. `span_first` is the
/// index of the first token of the match; `span_after` the index just past
/// its last token.
#[derive(Debug)]
struct SentenceView {
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
    "recent",
    "recurrent",
    "persistent",
    "intermittent",
    "chronic",
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
    let query_prefix = field_text[sentence_start..span_start]
        .trim_end()
        .ends_with('?');

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

    if tight_cue_applies(&view, NEGATION_PHRASES, TIGHT_GAP_ALLOW) {
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
                list_separator_after: false,
            });
        }

        cursor = token_end;
    }

    for index in 0..tokens.len().saturating_sub(1) {
        tokens[index].list_separator_after = has_list_separator_between(
            sentence,
            tokens[index].orig_end,
            tokens[index + 1].orig_start,
        );
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
        tokens,
        span_first,
        span_after: span_after.max(span_first),
    }
}

/// End index (exclusive) of the last phrase from `phrases` finishing at or
/// before `limit`.
fn last_phrase_end(tokens: &[Token], limit: usize, phrases: &[&[&str]]) -> Option<usize> {
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
                if last.map(|previous| end > previous).unwrap_or(true) {
                    last = Some(end);
                }
            }
        }
    }
    last
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
    gap.clone().all(|index| {
        let token = &tokens[index];
        !is_contrast(token)
            && (token.sibling
                || has_digit(token)
                || allow.contains(&token.text.as_str())
                || is_negated_list_fragment(tokens, index, match_start))
    })
}

fn is_negated_list_fragment(tokens: &[Token], index: usize, match_start: usize) -> bool {
    if index >= match_start {
        return false;
    }
    let token = &tokens[index];
    if token.text.chars().any(|ch| ch.is_ascii_digit()) || token.text.len() > 24 {
        return false;
    }
    if token.text.ends_with("ing") || token.text.ends_with("ed") {
        return false;
    }

    token.list_separator_after
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
        for text in [
            "No evidence of chest pain",
            "Denies any chest pain",
            "No new chest pain since",
        ] {
            let decision = classify(SoapField::History, text, "chest pain");
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
}
