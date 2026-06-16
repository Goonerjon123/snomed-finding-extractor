use crate::error::{ExtractorError, Result};
use crate::normalization::normalize_term;
use crate::terminology::{ConceptEntry, DescriptionEntry, TermVariant, TerminologyArtefact};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn build_from_openehr_valueset(path: impl AsRef<Path>) -> Result<TerminologyArtefact> {
    let bytes = fs::read(path.as_ref())?;
    let manifest: OpenEhrValueSetManifest = serde_json::from_slice(strip_utf8_bom(&bytes))?;
    let mut concepts = Vec::new();
    let mut seen = HashSet::new();

    for member in manifest.members.into_iter().filter(|member| member.active) {
        if member.system.as_deref() != Some("http://snomed.info/sct") {
            continue;
        }

        if !seen.insert(member.code.clone()) {
            continue;
        }

        let observable_entity = is_observable_entity(member.fsn.as_deref());
        let mut variants = Vec::new();
        let mut descriptions = Vec::new();
        push_variant(
            &mut variants,
            member.display.as_str(),
            "openehr-display",
            None,
            false,
        );
        if let Some(fsn) = member.fsn.as_deref() {
            if let Some(term) = strip_fsn_semantic_tag(fsn) {
                push_variant(
                    &mut variants,
                    term,
                    "openehr-fsn-without-semantic-tag",
                    None,
                    false,
                );
            }
        }
        for description in member
            .descriptions
            .into_iter()
            .filter(|description| description.active)
        {
            descriptions.push(DescriptionEntry {
                description_id: description.description_id.clone(),
                term: description.term.clone(),
                description_type: description.description_type.clone(),
                acceptability: description.acceptability.clone(),
                active: true,
            });

            push_variant(
                &mut variants,
                description.term.as_str(),
                &format!("openehr-description-{}", description.description_type),
                Some(description.description_id.clone()),
                false,
            );

            for derived in derive_description_variants(&description.term) {
                push_variant(
                    &mut variants,
                    derived.term.as_str(),
                    derived.source,
                    Some(description.description_id.clone()),
                    derived.allow_ambiguous,
                );
            }
        }
        if observable_entity {
            push_observable_entity_aliases(&mut variants, member.display.as_str());
            push_observable_numeric_label_variants(
                &mut variants,
                member.display.as_str(),
                &descriptions,
            );
        }

        concepts.push(ConceptEntry {
            concept_id: member.code,
            active: true,
            preferred_term: member.display,
            descriptions,
            variants,
        });
    }

    let mut artefact = TerminologyArtefact {
        schema_version: 1,
        terminology_version: manifest
            .terminology
            .version
            .unwrap_or_else(|| manifest.terminology.release_date.clone()),
        source_release: manifest.terminology.release_date,
        refset_id: manifest
            .value_set
            .rf2_refset_id
            .unwrap_or_else(|| manifest.id.clone()),
        generated_at_utc: unix_timestamp_string(),
        concepts,
        artefact_hash: String::new(),
    };
    artefact.artefact_hash = artefact.compute_hash()?;
    Ok(artefact)
}

#[derive(Debug, Clone)]
pub struct Rf2BuildInput<P> {
    pub concept_snapshot: P,
    pub description_snapshot: P,
    pub refset_snapshot: P,
    pub language_snapshot: Option<P>,
    pub refset_id: String,
    pub terminology_version: String,
    pub source_release: String,
}

pub fn build_from_rf2_snapshot<P: AsRef<Path>>(
    input: Rf2BuildInput<P>,
) -> Result<TerminologyArtefact> {
    let active_refset_members =
        read_active_refset_members(input.refset_snapshot, &input.refset_id)?;
    let active_concepts = read_active_concepts(input.concept_snapshot)?;
    let language_acceptability = match input.language_snapshot {
        Some(path) => read_language_acceptability(path)?,
        None => HashMap::new(),
    };

    let mut descriptions_by_concept: BTreeMap<String, Vec<DescriptionEntry>> = BTreeMap::new();
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(b'\t')
        .flexible(true)
        .from_path(input.description_snapshot)?;

    for row in reader.deserialize::<Rf2DescriptionRow>() {
        let row = row?;
        if row.active != "1" || !active_refset_members.contains(&row.concept_id) {
            continue;
        }
        if !active_concepts.contains(&row.concept_id) {
            continue;
        }

        descriptions_by_concept
            .entry(row.concept_id.clone())
            .or_default()
            .push(DescriptionEntry {
                description_id: row.id.clone(),
                term: row.term,
                description_type: description_type_name(&row.type_id).to_string(),
                acceptability: language_acceptability.get(&row.id).cloned(),
                active: true,
            });
    }

    let mut concepts = Vec::new();
    for concept_id in active_refset_members.iter() {
        if !active_concepts.contains(concept_id) {
            continue;
        }

        let descriptions = descriptions_by_concept
            .remove(concept_id)
            .unwrap_or_default();
        if descriptions.is_empty() {
            continue;
        }

        let preferred_term = choose_preferred_term(&descriptions);
        let mut variants = Vec::new();
        for description in descriptions
            .iter()
            .filter(|description| description.description_type != "fully_specified_name")
        {
            push_variant(
                &mut variants,
                description.term.as_str(),
                &format!("rf2-{}", description.description_type),
                Some(description.description_id.clone()),
                false,
            );

            for derived in derive_description_variants(&description.term) {
                push_variant(
                    &mut variants,
                    derived.term.as_str(),
                    derived.source,
                    Some(description.description_id.clone()),
                    derived.allow_ambiguous,
                );
            }
        }

        concepts.push(ConceptEntry {
            concept_id: concept_id.clone(),
            active: true,
            preferred_term,
            descriptions,
            variants,
        });
    }

    let mut artefact = TerminologyArtefact {
        schema_version: 1,
        terminology_version: input.terminology_version,
        source_release: input.source_release,
        refset_id: input.refset_id,
        generated_at_utc: unix_timestamp_string(),
        concepts,
        artefact_hash: String::new(),
    };
    artefact.artefact_hash = artefact.compute_hash()?;
    Ok(artefact)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenEhrValueSetManifest {
    id: String,
    terminology: OpenEhrTerminology,
    value_set: OpenEhrValueSet,
    members: Vec<OpenEhrMember>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenEhrTerminology {
    #[serde(default)]
    version: Option<String>,
    release_date: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenEhrValueSet {
    #[serde(default)]
    rf2_refset_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenEhrMember {
    code: String,
    display: String,
    #[serde(default)]
    fsn: Option<String>,
    active: bool,
    #[serde(default)]
    system: Option<String>,
    #[serde(default)]
    descriptions: Vec<OpenEhrDescription>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenEhrDescription {
    description_id: String,
    term: String,
    #[serde(rename = "type")]
    description_type: String,
    #[serde(default)]
    acceptability: Option<String>,
    active: bool,
}

#[derive(Debug, Deserialize)]
struct Rf2ConceptRow {
    id: String,
    active: String,
}

#[derive(Debug, Deserialize)]
struct Rf2DescriptionRow {
    id: String,
    active: String,
    #[serde(rename = "conceptId")]
    concept_id: String,
    term: String,
    #[serde(rename = "typeId")]
    type_id: String,
}

#[derive(Debug, Deserialize)]
struct Rf2RefsetRow {
    active: String,
    #[serde(rename = "refsetId")]
    refset_id: String,
    #[serde(rename = "referencedComponentId")]
    referenced_component_id: String,
}

#[derive(Debug, Deserialize)]
struct Rf2LanguageRow {
    active: String,
    #[serde(rename = "referencedComponentId")]
    referenced_component_id: String,
    #[serde(rename = "acceptabilityId")]
    acceptability_id: String,
}

fn read_active_concepts(path: impl AsRef<Path>) -> Result<HashSet<String>> {
    let mut concepts = HashSet::new();
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(b'\t')
        .flexible(true)
        .from_path(path)?;

    for row in reader.deserialize::<Rf2ConceptRow>() {
        let row = row?;
        if row.active == "1" {
            concepts.insert(row.id);
        }
    }

    Ok(concepts)
}

fn read_active_refset_members(path: impl AsRef<Path>, refset_id: &str) -> Result<BTreeSet<String>> {
    let mut members = BTreeSet::new();
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(b'\t')
        .flexible(true)
        .from_path(path)?;

    for row in reader.deserialize::<Rf2RefsetRow>() {
        let row = row?;
        if row.active == "1" && row.refset_id == refset_id {
            members.insert(row.referenced_component_id);
        }
    }

    if members.is_empty() {
        return Err(ExtractorError::InvalidInput(format!(
            "no active members found for refset {refset_id}"
        )));
    }

    Ok(members)
}

fn read_language_acceptability(path: impl AsRef<Path>) -> Result<HashMap<String, String>> {
    let mut acceptability = HashMap::new();
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(b'\t')
        .flexible(true)
        .from_path(path)?;

    for row in reader.deserialize::<Rf2LanguageRow>() {
        let row = row?;
        if row.active == "1" {
            acceptability.insert(
                row.referenced_component_id,
                acceptability_name(&row.acceptability_id).to_string(),
            );
        }
    }

    Ok(acceptability)
}

fn choose_preferred_term(descriptions: &[DescriptionEntry]) -> String {
    descriptions
        .iter()
        .find(|description| {
            description.description_type == "synonym"
                && description.acceptability.as_deref() == Some("preferred")
        })
        .or_else(|| {
            descriptions
                .iter()
                .find(|description| description.description_type == "synonym")
        })
        .or_else(|| descriptions.first())
        .map(|description| {
            strip_fsn_semantic_tag(&description.term)
                .unwrap_or(&description.term)
                .to_string()
        })
        .unwrap_or_default()
}

fn push_variant(
    variants: &mut Vec<TermVariant>,
    text: &str,
    source: &str,
    description_id: Option<String>,
    allow_ambiguous: bool,
) {
    push_variant_with_numeric_requirement(
        variants,
        text,
        source,
        description_id,
        allow_ambiguous,
        false,
    );
}

fn push_numeric_value_variant(
    variants: &mut Vec<TermVariant>,
    text: &str,
    source: &str,
    description_id: Option<String>,
) {
    push_variant_with_numeric_requirement(variants, text, source, description_id, true, true);
}

fn push_variant_with_numeric_requirement(
    variants: &mut Vec<TermVariant>,
    text: &str,
    source: &str,
    description_id: Option<String>,
    allow_ambiguous: bool,
    requires_numeric_value: bool,
) {
    if normalize_term(text).is_empty() {
        return;
    }
    if variants.iter().any(|variant| {
        normalize_term(&variant.text) == normalize_term(text)
            && variant.requires_numeric_value == requires_numeric_value
    }) {
        return;
    }

    variants.push(TermVariant {
        text: text.to_string(),
        source: source.to_string(),
        description_id,
        allow_ambiguous,
        requires_numeric_value,
    });
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DerivedVariant {
    term: String,
    source: &'static str,
    allow_ambiguous: bool,
}

fn derive_description_variants(term: &str) -> Vec<DerivedVariant> {
    let mut variants = Vec::new();
    if let Some((prefix, expansion)) = split_acronym_expansion(term) {
        variants.push(DerivedVariant {
            term: expansion.to_string(),
            source: "openehr-description-expansion",
            allow_ambiguous: false,
        });
        if let Some(short_of_breath) = shortness_of_breath_variant(expansion) {
            variants.push(DerivedVariant {
                term: short_of_breath,
                source: "openehr-description-phrase-variant",
                allow_ambiguous: false,
            });
        }

        for diabetes_variant in simple_diabetes_mellitus_variants(expansion) {
            variants.push(DerivedVariant {
                term: diabetes_variant,
                source: "openehr-description-diabetes-mellitus-variant",
                allow_ambiguous: false,
            });
        }

        if acronym_matches_expansion(prefix, expansion) {
            variants.push(DerivedVariant {
                term: prefix.to_string(),
                source: "openehr-description-acronym",
                allow_ambiguous: true,
            });
        } else if is_safe_non_initialism_acronym_prefix(prefix, expansion) {
            variants.push(DerivedVariant {
                term: prefix.to_string(),
                source: "openehr-description-acronym-prefix",
                allow_ambiguous: true,
            });
        }
    }

    if let Some(short_of_breath) = shortness_of_breath_variant(term) {
        variants.push(DerivedVariant {
            term: short_of_breath,
            source: "openehr-description-phrase-variant",
            allow_ambiguous: false,
        });
    }

    for diabetes_variant in simple_diabetes_mellitus_variants(term) {
        variants.push(DerivedVariant {
            term: diabetes_variant,
            source: "openehr-description-diabetes-mellitus-variant",
            allow_ambiguous: false,
        });
    }

    for morphology_variant in morphology_variants(term) {
        variants.push(DerivedVariant {
            term: morphology_variant,
            source: "openehr-description-morphology-variant",
            allow_ambiguous: false,
        });
    }

    for phrase_variant in clinical_phrase_variants(term) {
        variants.push(DerivedVariant {
            term: phrase_variant,
            source: "openehr-description-clinical-phrase-variant",
            allow_ambiguous: false,
        });
    }

    for context_trimmed in context_suffix_trim_variants(term) {
        variants.push(DerivedVariant {
            term: context_trimmed,
            source: "openehr-description-context-trim",
            allow_ambiguous: false,
        });
    }

    variants
}

fn push_observable_entity_aliases(variants: &mut Vec<TermVariant>, preferred_term: &str) {
    let normalized_preferred = normalize_term(preferred_term);
    let aliases: &[(&str, bool)] = match normalized_preferred.as_str() {
        "blood pressure" => &[("BP", true)],
        "heart rate" => &[("HR", true)],
        "pulse rate" => &[("PR", true)],
        "respiratory rate" => &[("RR", true), ("BR", true), ("resp rate", false)],
        "peripheral oxygen saturation" => &[
            ("SpO2", true),
            ("sats", false),
            ("O2 sats", false),
            ("oxygen sats", false),
        ],
        "haemoglobin saturation with oxygen" | "hemoglobin saturation with oxygen" => {
            &[("oxygen saturation", false)]
        }
        "body temperature" => &[("temp", false), ("temperature", false), ("BT", true)],
        "body mass index" => &[("BMI", true)],
        _ => &[],
    };

    for (alias, allow_ambiguous) in aliases {
        push_variant(
            variants,
            alias,
            "built-in-observable-alias",
            None,
            *allow_ambiguous,
        );
    }

    if normalized_preferred == "body temperature" {
        for alias in ["afeb", "afebrile", "apyrexial"] {
            push_numeric_value_variant(variants, alias, "built-in-observable-numeric-alias", None);
        }
    }
}

fn push_observable_numeric_label_variants(
    variants: &mut Vec<TermVariant>,
    preferred_term: &str,
    descriptions: &[DescriptionEntry],
) {
    for label in observable_numeric_labels(preferred_term) {
        push_numeric_value_variant(variants, &label, "openehr-observable-numeric-label", None);
    }

    for description in descriptions {
        for label in observable_numeric_labels(&description.term) {
            push_numeric_value_variant(
                variants,
                &label,
                "openehr-observable-numeric-label",
                Some(description.description_id.clone()),
            );
        }
    }
}

fn observable_numeric_labels(term: &str) -> Vec<String> {
    let mut labels = Vec::new();
    labels.extend(simple_rate_numeric_labels(term));
    labels.extend(acronym_expansion_numeric_labels(term));
    let mut seen = HashSet::new();
    labels
        .into_iter()
        .filter(|label| seen.insert(normalize_term(label)))
        .collect()
}

fn simple_rate_numeric_labels(term: &str) -> Vec<String> {
    let normalized = normalize_term(term);
    let words = normalized.split(' ').collect::<Vec<_>>();
    if words.len() != 2 || words[1] != "rate" || words[0].chars().count() < 4 {
        return Vec::new();
    }

    let mut labels = vec![capitalize_label(words[0])];
    if let Some(initial) = words[0].chars().next() {
        labels.push(initial.to_ascii_uppercase().to_string());
    }
    labels
}

fn acronym_expansion_numeric_labels(term: &str) -> Vec<String> {
    let Some((prefix, expansion)) = split_observable_acronym_expansion(term) else {
        return Vec::new();
    };
    if !acronym_matches_expansion(prefix, expansion) {
        return Vec::new();
    }

    let normalized_expansion = normalize_term(expansion);
    let words = normalized_expansion.split(' ').collect::<Vec<_>>();
    if words.len() == 2 && words[1] == "temperature" && words[0].chars().count() >= 4 {
        return vec!["T".to_string()];
    }

    Vec::new()
}

fn split_observable_acronym_expansion(term: &str) -> Option<(&str, &str)> {
    let (prefix, expansion) = term.split_once(" - ")?;
    let prefix = prefix.trim();
    let expansion = expansion.trim();
    if prefix.len() < 2 || prefix.len() > 12 || expansion.len() < 5 {
        return None;
    }
    if !is_acronym_like_prefix(prefix) {
        return None;
    }
    Some((prefix, expansion))
}

fn capitalize_label(label: &str) -> String {
    let mut chars = label.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };

    format!("{}{}", first.to_ascii_uppercase(), chars.as_str())
}

fn split_acronym_expansion(term: &str) -> Option<(&str, &str)> {
    let (prefix, expansion) = term.split_once(" - ")?;
    let prefix = prefix.trim();
    let expansion = expansion.trim();
    if prefix.len() < 3 || prefix.len() > 12 || expansion.len() < 5 {
        return None;
    }
    if !is_acronym_like_prefix(prefix) {
        return None;
    }
    Some((prefix, expansion))
}

fn is_acronym_like_prefix(prefix: &str) -> bool {
    if !prefix.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return false;
    }

    let upper_count = prefix.chars().filter(|ch| ch.is_ascii_uppercase()).count();
    let letter_count = prefix.chars().filter(|ch| ch.is_ascii_alphabetic()).count();

    upper_count >= 2 || (prefix.chars().any(|ch| ch.is_ascii_digit()) && letter_count > 0)
}

fn acronym_matches_expansion(prefix: &str, expansion: &str) -> bool {
    let acronym = prefix
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect::<String>();
    let initials = normalize_term(expansion)
        .split(' ')
        .filter_map(|word| word.chars().next())
        .collect::<String>();
    acronym == initials
}

fn is_safe_non_initialism_acronym_prefix(prefix: &str, expansion: &str) -> bool {
    !expansion_starts_with_unencoded_specificity(prefix, expansion)
}

fn expansion_starts_with_unencoded_specificity(prefix: &str, expansion: &str) -> bool {
    let normalized_prefix = normalize_term(prefix);
    let normalized_expansion = normalize_term(expansion);
    let Some(first_word) = normalized_expansion.split(' ').next() else {
        return false;
    };

    let specificity_words = [
        "acute",
        "bacterial",
        "bilateral",
        "chronic",
        "left",
        "mild",
        "moderate",
        "recurrent",
        "right",
        "severe",
        "viral",
    ];
    if !specificity_words.contains(&first_word) {
        return false;
    }

    let Some(first_letter) = first_word.chars().next() else {
        return false;
    };
    !normalized_prefix.starts_with(first_letter)
}

fn simple_diabetes_mellitus_variants(term: &str) -> Vec<String> {
    let normalized = normalize_term(term);
    let Some(type_suffix) = normalized.strip_prefix("type ") else {
        return diabetes_mellitus_type_suffix_variants(&normalized);
    };

    let Some(type_code) = type_suffix.strip_suffix(" diabetes mellitus") else {
        return Vec::new();
    };
    let Some(type_label) = diabetes_type_label(type_code) else {
        return Vec::new();
    };

    vec![format!("Type {type_label} diabetes")]
}

fn diabetes_mellitus_type_suffix_variants(normalized: &str) -> Vec<String> {
    let Some(type_code) = normalized.strip_prefix("diabetes mellitus type ") else {
        return Vec::new();
    };
    let Some(type_label) = diabetes_type_label(type_code) else {
        return Vec::new();
    };

    vec![
        format!("Type {type_label} diabetes"),
        format!("Diabetes type {type_label}"),
    ]
}

fn diabetes_type_label(type_code: &str) -> Option<&'static str> {
    match type_code {
        "1" => Some("1"),
        "2" => Some("2"),
        "i" => Some("I"),
        "ii" => Some("II"),
        _ => None,
    }
}

fn morphology_variants(term: &str) -> Vec<String> {
    let normalized = normalize_term(term);
    if let Some(body_site) = normalized.strip_prefix("swelling of ") {
        return vec![format!("swollen {}", body_site.trim())];
    }
    if let Some(body_site) = normalized.strip_suffix(" swelling") {
        let body_site = body_site.trim();
        if !body_site.is_empty() {
            return vec![format!("swollen {body_site}")];
        }
    }
    for suffix in [" edema", " oedema"] {
        if let Some(body_site) = normalized.strip_suffix(suffix) {
            let body_site = body_site.trim();
            if safe_short_body_site_phrase(body_site) {
                return vec![
                    format!("{body_site} swelling"),
                    format!("swollen {body_site}"),
                ];
            }
        }
    }

    let normalized = normalized
        .strip_suffix(" symptom")
        .map(str::trim)
        .unwrap_or(normalized.as_str());
    if let Some(body_site) = normalized.strip_prefix("stiff ") {
        let body_site = body_site.trim();
        if safe_short_body_site_phrase(body_site) {
            return vec![format!("{body_site} stiffness")];
        }
    }
    if let Some(body_site) = normalized.strip_suffix(" stiffness") {
        let body_site = body_site.trim();
        if safe_short_body_site_phrase(body_site) {
            return vec![format!("stiff {body_site}")];
        }
    }

    Vec::new()
}

fn clinical_phrase_variants(term: &str) -> Vec<String> {
    let mut variants = Vec::new();
    if let Some((_, suffix)) = term.split_once(" - ") {
        let suffix = normalize_term(suffix);
        if is_safe_context_trimmed_phrase(&suffix) {
            variants.push(suffix);
        }
    }

    let normalized = normalize_term(term);
    let normalized = normalized
        .strip_prefix("finding of ")
        .or_else(|| normalized.strip_prefix("observation of "))
        .unwrap_or(normalized.as_str());

    variants.extend(coordinator_omission_variants(normalized));
    variants.extend(pain_phrase_variants(normalized));
    variants.extend(cold_body_site_variants(normalized));
    variants.extend(colloquial_symptom_variants(normalized));

    if let Some(function) = normalized.strip_prefix("impaired ") {
        let function = function.trim();
        if safe_short_body_site_phrase(function) {
            variants.push(format!("reduced {function}"));
        }
    }
    if let Some(function) = normalized.strip_suffix(" impairment") {
        let function = function.trim();
        if safe_short_body_site_phrase(function) {
            variants.push(format!("reduced {function}"));
        }
    }
    if normalized == "depressed mood" {
        variants.push("low mood".to_string());
    }
    if let Some(base) = normalized.strip_suffix(" symptom") {
        let base = base.trim();
        if is_safe_clinical_phrase_variant(base) {
            variants.push(base.to_string());
        }
    }
    if let Some(base) = normalized.strip_suffix(" not associated with childbirth") {
        let base = base.trim();
        if is_safe_clinical_phrase_variant(base) {
            variants.push(base.to_string());
        }
    }
    if normalized == "period pain" {
        variants.push("painful periods".to_string());
    }
    if matches!(
        normalized,
        "urgency urination"
            | "urgency of micturition"
            | "urgency to micturate"
            | "urgency to pass urine"
            | "urinary precipitancy"
            | "urgent desire to urinate"
    ) {
        variants.push("urgency".to_string());
    }
    if let Some(body_site) = normalized
        .strip_prefix("discharge from ")
        .or_else(|| normalized.strip_prefix("discharge of "))
    {
        let body_site = body_site.trim();
        if safe_short_body_site_phrase(body_site) {
            variants.push(format!("{body_site} discharge"));
        }
    }

    match normalized {
        "frequency of urination" | "frequency of micturition" => {
            variants.push("urinary frequency".to_string());
            variants.push("passing urine more often".to_string());
            variants.push("urinating more often".to_string());
            variants.push("going more often".to_string());
        }
        _ => {}
    }

    variants
}

fn coordinator_omission_variants(normalized: &str) -> Vec<String> {
    let words = normalized.split(' ').collect::<Vec<_>>();
    if words.len() < 3 || words.len() > 5 || !words.contains(&"and") {
        return Vec::new();
    }
    if !words
        .iter()
        .all(|word| *word == "and" || word.chars().all(|ch| ch.is_ascii_alphabetic()))
    {
        return Vec::new();
    }

    let without_and = words
        .iter()
        .filter(|word| **word != "and")
        .copied()
        .collect::<Vec<_>>();
    if without_and.len() < 2
        || without_and
            .iter()
            .any(|word| word.chars().filter(|ch| ch.is_alphanumeric()).count() < 4)
    {
        return Vec::new();
    }

    vec![without_and.join(" ")]
}

fn pain_phrase_variants(normalized: &str) -> Vec<String> {
    let mut variants = Vec::new();

    if let Some(body_site) = normalized.strip_suffix(" pain") {
        let body_site = body_site.trim();
        if safe_short_body_site_phrase(body_site) {
            if body_site.split(' ').count() > 1 {
                variants.push(format!("pain {body_site}"));
            }
            variants.push(format!("{body_site} discomfort"));
        }
    }

    for prefix in ["pain in ", "pain of "] {
        if let Some(body_site) = normalized.strip_prefix(prefix) {
            let body_site = body_site.trim();
            if safe_short_body_site_phrase(body_site) {
                variants.push(format!("{body_site} pain"));
                variants.push(format!("{body_site} discomfort"));
                variants.push(format!("discomfort {body_site}"));
            }
        }
    }

    variants
}

fn cold_body_site_variants(normalized: &str) -> Vec<String> {
    let mut variants = Vec::new();

    let Some(body_site) = normalized.strip_prefix("cold ") else {
        return variants;
    };
    let body_site = body_site.trim();
    if safe_short_body_site_phrase(body_site) {
        variants.push(format!("{body_site} cold"));
        variants.push(format!("{body_site} feel cold"));
        variants.push(format!("{body_site} feels cold"));
    }

    variants
}

fn colloquial_symptom_variants(normalized: &str) -> Vec<String> {
    let mut variants = Vec::new();

    for prefix in ["feeling ", "feels "] {
        if let Some(base) = normalized.strip_prefix(prefix) {
            let base = base.trim();
            if is_safe_clinical_phrase_variant(base) {
                variants.push(base.to_string());
            }
        }
    }

    match normalized {
        "abdominal bloating" => {
            variants.push("bloating".to_string());
            variants.push("bloated".to_string());
        }
        "abdominal pain" => {
            variants.push("abdominal cramps".to_string());
            variants.push("abdominal cramp".to_string());
            variants.push("abdominal cramping".to_string());
        }
        "lower abdominal pain" => {
            variants.push("lower abdominal cramps".to_string());
            variants.push("lower abdominal cramp".to_string());
            variants.push("lower abdominal cramping".to_string());
            variants.push("lower abdomen cramps".to_string());
            variants.push("lower abdomen cramping".to_string());
        }
        "alopecia" | "loss of hair" => {
            variants.push("hair thinning".to_string());
            variants.push("thinning hair".to_string());
        }
        "breath smells unpleasant" => {
            variants.push("bad breath".to_string());
        }
        "crust" | "crusting of skin" => {
            variants.push("crusted".to_string());
            variants.push("skin crusted".to_string());
        }
        "cervical lymphadenopathy" => {
            variants.push("swollen neck glands".to_string());
            variants.push("swollen glands in neck".to_string());
            variants.push("neck glands swollen".to_string());
        }
        "dry eyes" => {
            variants.push("dry eye".to_string());
            variants.push("eye feels dry".to_string());
            variants.push("eyes feel dry".to_string());
        }
        "dysuria" | "painful micturition" | "painful urination" => {
            variants.push("burning on passing urine".to_string());
            variants.push("burning when passing urine".to_string());
            variants.push("burning to pass urine".to_string());
            variants.push("burning urination".to_string());
        }
        "dyssomnia" | "sleep disturbance" => {
            variants.push("disturbed sleep".to_string());
            variants.push("poor sleep".to_string());
            variants.push("struggling to sleep".to_string());
            variants.push("keeping awake".to_string());
        }
        "erythema" => {
            variants.push("redness".to_string());
            variants.push("red hot".to_string());
            variants.push("red and hot".to_string());
            variants.push("red swollen".to_string());
            variants.push("red and swollen".to_string());
        }
        "excessive sweating" => {
            variants.push("sweaty".to_string());
        }
        "fatigue" => {
            variants.push("tired".to_string());
            variants.push("tiredness".to_string());
            variants.push("exhausted".to_string());
            variants.push("no energy".to_string());
            variants.push("low energy".to_string());
        }
        "foreign body sensation" => {
            variants.push("gritty eye".to_string());
            variants.push("eye feels gritty".to_string());
            variants.push("gritty eyes".to_string());
        }
        "generalised aches and pains" | "generalized aches and pains" => {
            variants.push("aching all over".to_string());
            variants.push("aches all over".to_string());
            variants.push("achy".to_string());
        }
        "heavy menstrual bleeding" | "menorrhagia" => {
            variants.push("heavy periods".to_string());
            variants.push("heavier periods".to_string());
            variants.push("periods heavier".to_string());
        }
        "hot skin" => {
            variants.push("skin hot".to_string());
            variants.push("hot swollen".to_string());
            variants.push("hot and swollen".to_string());
            variants.push("hot red".to_string());
            variants.push("hot and red".to_string());
        }
        "initial insomnia" | "difficulty falling asleep" => {
            variants.push("takes ages to drop off".to_string());
            variants.push("difficulty dropping off".to_string());
        }
        "intolerant of cold" => {
            variants.push("cold intolerance".to_string());
            variants.push("cold all the time".to_string());
        }
        "joint crepitus" => {
            variants.push("grinding".to_string());
            variants.push("creaking".to_string());
            variants.push("grinding joint".to_string());
            variants.push("creaking joint".to_string());
        }
        "low back pain" => {
            variants.push("back pain".to_string());
        }
        "malodorous urine" => {
            variants.push("strong smelling".to_string());
            variants.push("strong smelling urine".to_string());
            variants.push("urine strong smelling".to_string());
            variants.push("urine smells strong".to_string());
            variants.push("foul smelling".to_string());
            variants.push("offensive smelling".to_string());
            variants.push("offensive smelling urine".to_string());
            variants.push("smelly".to_string());
            variants.push("smelly urine".to_string());
        }
        "malaise" => {
            variants.push("unwell".to_string());
            variants.push("generally unwell".to_string());
            variants.push("feeling unwell".to_string());
        }
        "nasal congestion" => {
            variants.push("blocked nose".to_string());
            variants.push("stuffy nose".to_string());
        }
        "nasal discharge" | "anterior rhinorrhea" | "anterior rhinorrhoea" => {
            variants.push("runny nose".to_string());
            variants.push("watery nasal discharge".to_string());
            variants.push("watery discharge".to_string());
        }
        "nocturia" => {}
        "pain in pelvis" => {
            variants.push("pelvic ache".to_string());
            variants.push("pelvic discomfort".to_string());
            variants.push("dragging pelvic ache".to_string());
        }
        "palpitations" => {
            variants.push("heart races".to_string());
            variants.push("heart racing".to_string());
            variants.push("racing heartbeat".to_string());
        }
        "poor stream of urine" => {
            variants.push("poor flow".to_string());
            variants.push("poor urinary flow".to_string());
            variants.push("weak urinary stream".to_string());
        }
        "postural lightheadedness" => {
            variants.push("lightheaded on standing".to_string());
            variants.push("lightheadedness on standing".to_string());
            variants.push("light headed on standing".to_string());
        }
        "racing thoughts" => {
            variants.push("mind racing".to_string());
        }
        "recurrent falls" => {
            variants.push("falls".to_string());
            variants.push("repeated falls".to_string());
        }
        "shiny skin" => {
            variants.push("skin shiny".to_string());
            variants.push("skin tight shiny".to_string());
            variants.push("tight shiny skin".to_string());
        }
        "swelling" => {
            variants.push("swollen".to_string());
        }
        "swallowing painful" => {
            variants.push("painful swallowing".to_string());
        }
        "sensation as if urinary bladder still full"
        | "incomplete emptying of bladder"
        | "incomplete emptying of urinary bladder" => {
            variants.push("bladder not empty".to_string());
            variants.push("feels bladder not empty".to_string());
            variants.push("bladder still full".to_string());
        }
        "terminal dribbling of urine" => {
            variants.push("terminal dribbling".to_string());
        }
        "tenderness" => {
            variants.push("tender".to_string());
            variants.push("exquisitely tender".to_string());
            variants.push("tender to touch".to_string());
            variants.push("painful to touch".to_string());
        }
        "taste sense altered" => {
            variants.push("taste altered".to_string());
            variants.push("altered taste".to_string());
        }
        "tight chest" => {
            variants.push("chest tight".to_string());
            variants.push("chest tightness".to_string());
        }
        "tremor" => {
            variants.push("shaky".to_string());
            variants.push("feels shaky".to_string());
            variants.push("shaking".to_string());
        }
        "unsteady when walking" | "general unsteadiness" => {
            variants.push("unsteady".to_string());
            variants.push("unsteadiness".to_string());
        }
        "urgent desire for stool" => {
            variants.push("bowel urgency".to_string());
            variants.push("fecal urgency".to_string());
            variants.push("faecal urgency".to_string());
        }
        "weakness of face muscles" => {
            variants.push("facial droop".to_string());
            variants.push("face drooped".to_string());
            variants.push("side of face drooped".to_string());
            variants.push("cannot smile".to_string());
            variants.push("can't smile".to_string());
            variants.push("cannot close eye".to_string());
            variants.push("can't close eye".to_string());
        }
        "weight increased" | "weight gain" | "abnormal weight gain" => {
            variants.push("gained weight".to_string());
            variants.push("gaining weight".to_string());
            variants.push("put on weight".to_string());
        }
        _ => {}
    }

    variants
}

fn safe_short_body_site_phrase(value: &str) -> bool {
    let words = value
        .split(' ')
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    !words.is_empty()
        && words.len() <= 4
        && words
            .iter()
            .all(|word| word.chars().all(|ch| ch.is_ascii_alphabetic()))
}

fn is_safe_clinical_phrase_variant(value: &str) -> bool {
    let words = value
        .split(' ')
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    let alnum_count = value.chars().filter(|ch| ch.is_alphanumeric()).count();

    !words.is_empty()
        && words.len() <= 5
        && alnum_count >= 6
        && words
            .iter()
            .all(|word| word.chars().all(|ch| ch.is_ascii_alphabetic()))
}

fn context_suffix_trim_variants(term: &str) -> Vec<String> {
    let normalized = normalize_term(term);
    let Some(base) = normalized.strip_suffix(" on auscultation") else {
        return Vec::new();
    };
    let base = base.trim();
    if !is_safe_context_trimmed_phrase(base) {
        return Vec::new();
    }

    vec![base.to_string()]
}

fn is_safe_context_trimmed_phrase(normalized: &str) -> bool {
    let word_count = normalized
        .split(' ')
        .filter(|word| !word.is_empty())
        .count();
    let alnum_count = normalized.chars().filter(|ch| ch.is_alphanumeric()).count();

    word_count >= 2 && alnum_count >= 6
}

fn shortness_of_breath_variant(term: &str) -> Option<String> {
    let normalized = normalize_term(term);
    if normalized == "shortness of breath" {
        return Some("short of breath".to_string());
    }

    normalized
        .strip_prefix("shortness of breath ")
        .map(|suffix| format!("short of breath {suffix}"))
}

fn strip_fsn_semantic_tag(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    let idx = trimmed.rfind(" (")?;
    Some(trimmed[..idx].trim())
}

fn is_observable_entity(fsn: Option<&str>) -> bool {
    fsn.map(|value| {
        value
            .trim()
            .to_ascii_lowercase()
            .ends_with("(observable entity)")
    })
    .unwrap_or(false)
}

fn description_type_name(type_id: &str) -> &'static str {
    match type_id {
        "900000000000003001" => "fully_specified_name",
        "900000000000013009" => "synonym",
        "900000000000550004" => "definition",
        _ => "other",
    }
}

fn acceptability_name(acceptability_id: &str) -> &'static str {
    match acceptability_id {
        "900000000000548007" => "preferred",
        "900000000000549004" => "acceptable",
        _ => "unknown",
    }
}

fn strip_utf8_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes)
}

fn unix_timestamp_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| format!("unix:{}", duration.as_secs()))
        .unwrap_or_else(|_| "unix:0".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_fsn_tag() {
        assert_eq!(
            strip_fsn_semantic_tag("Chest pain (finding)"),
            Some("Chest pain")
        );
    }

    #[test]
    fn strips_utf8_bom() {
        assert_eq!(strip_utf8_bom(&[0xef, 0xbb, 0xbf, b'{']), b"{");
    }

    #[test]
    fn derives_acronym_and_phrase_variants_from_official_descriptions() {
        let derived = derive_description_variants("SOBOE - Shortness of breath on exertion");

        assert!(derived.iter().any(|variant| variant.term == "SOBOE"
            && variant.source == "openehr-description-acronym"
            && variant.allow_ambiguous));
        assert!(derived
            .iter()
            .any(|variant| variant.term == "Shortness of breath on exertion"));
        assert!(derived
            .iter()
            .any(|variant| variant.term == "short of breath on exertion"));
    }

    #[test]
    fn does_not_derive_expansions_from_plain_hyphenated_descriptions() {
        let derived = derive_description_variants("Spasm - movement");

        assert!(!derived.iter().any(|variant| variant.term == "movement"
            && variant.source == "openehr-description-expansion"));
    }

    #[test]
    fn derives_non_initialism_acronym_prefixes_from_official_descriptions() {
        let derived = derive_description_variants("T2DM - diabetes mellitus type 2");

        assert!(derived.iter().any(|variant| variant.term == "T2DM"
            && variant.source == "openehr-description-acronym-prefix"
            && variant.allow_ambiguous));
        assert!(derived
            .iter()
            .any(|variant| variant.term == "Type 2 diabetes"
                && variant.source == "openehr-description-diabetes-mellitus-variant"));
    }

    #[test]
    fn derives_clinical_acronym_prefixes_even_when_word_order_differs() {
        let derived =
            derive_description_variants("URTI - Infection of the upper respiratory tract");

        assert!(derived.iter().any(|variant| variant.term == "URTI"
            && variant.source == "openehr-description-acronym-prefix"
            && variant.allow_ambiguous));
    }

    #[test]
    fn does_not_strip_unencoded_specificity_from_acronym_expansions() {
        let derived = derive_description_variants("URTI - Viral upper respiratory tract infection");

        assert!(!derived
            .iter()
            .any(|variant| variant.term == "URTI" && variant.allow_ambiguous));
    }

    #[test]
    fn derives_safe_context_trimmed_examination_phrases() {
        let derived = derive_description_variants("Chest clear on auscultation");

        assert!(derived.iter().any(|variant| variant.term == "chest clear"
            && variant.source == "openehr-description-context-trim"
            && !variant.allow_ambiguous));
    }

    #[test]
    fn derives_morphology_variants_from_body_site_signs() {
        let derived = derive_description_variants("Swelling of left tonsil");

        assert!(derived
            .iter()
            .any(|variant| variant.term == "swollen left tonsil"
                && variant.source == "openehr-description-morphology-variant"
                && !variant.allow_ambiguous));
    }

    #[test]
    fn derives_stiffness_variants_from_stiff_body_site_terms() {
        let derived = derive_description_variants("Stiff neck symptom");

        assert!(derived
            .iter()
            .any(|variant| variant.term == "neck stiffness"
                && variant.source == "openehr-description-morphology-variant"
                && !variant.allow_ambiguous));
    }

    #[test]
    fn derives_swelling_variants_from_edema_terms() {
        let derived = derive_description_variants("Ankle edema");

        assert!(derived
            .iter()
            .any(|variant| variant.term == "ankle swelling"
                && variant.source == "openehr-description-morphology-variant"
                && !variant.allow_ambiguous));
        assert!(derived.iter().any(|variant| variant.term == "swollen ankle"
            && variant.source == "openehr-description-morphology-variant"
            && !variant.allow_ambiguous));
    }

    #[test]
    fn derives_common_clinical_phrase_variants_from_official_descriptions() {
        let derived = derive_description_variants("Frequency of urination");

        assert!(derived
            .iter()
            .any(|variant| variant.term == "urinary frequency"
                && variant.source == "openehr-description-clinical-phrase-variant"
                && !variant.allow_ambiguous));
    }

    #[test]
    fn derives_safe_hyphen_suffix_phrases() {
        let derived = derive_description_variants("Shoulder joint - painful arc");

        assert!(derived.iter().any(|variant| variant.term == "painful arc"
            && variant.source == "openehr-description-clinical-phrase-variant"
            && !variant.allow_ambiguous));

        let unsafe_derived = derive_description_variants("Spasm - movement");
        assert!(!unsafe_derived
            .iter()
            .any(|variant| variant.term == "movement"));
    }

    #[test]
    fn derives_plain_phrase_from_symptom_suffix_terms() {
        let derived = derive_description_variants("Belching symptom");

        assert!(derived.iter().any(|variant| variant.term == "belching"
            && variant.source == "openehr-description-clinical-phrase-variant"
            && !variant.allow_ambiguous));
    }

    #[test]
    fn derives_common_urinary_and_gynae_phrase_variants() {
        let urgency = derive_description_variants("Urgency - urination");
        assert!(urgency.iter().any(|variant| variant.term == "urgency"
            && variant.source == "openehr-description-clinical-phrase-variant"
            && !variant.allow_ambiguous));

        let dysmenorrhea = derive_description_variants("Period pain");
        assert!(dysmenorrhea
            .iter()
            .any(|variant| variant.term == "painful periods"
                && variant.source == "openehr-description-clinical-phrase-variant"
                && !variant.allow_ambiguous));
    }

    #[test]
    fn derives_subjective_gp_phrase_variants() {
        let dysuria = derive_description_variants("Dysuria");
        assert!(dysuria
            .iter()
            .any(|variant| variant.term == "burning on passing urine"));

        let pain = derive_description_variants("Suprapubic pain");
        assert!(pain
            .iter()
            .any(|variant| variant.term == "suprapubic discomfort"));

        let abdominal_pain = derive_description_variants("Abdominal pain");
        assert!(abdominal_pain
            .iter()
            .any(|variant| variant.term == "abdominal cramping"));

        let lower_abdominal_pain = derive_description_variants("Lower abdominal pain");
        assert!(lower_abdominal_pain
            .iter()
            .any(|variant| variant.term == "lower abdominal cramping"));

        let abdominal_colic = derive_description_variants("Abdominal colic");
        assert!(!abdominal_colic
            .iter()
            .any(|variant| variant.term == "abdominal cramps"));

        let head_pain = derive_description_variants("Head pain");
        assert!(!head_pain.iter().any(|variant| variant.term == "pain head"));

        let malodorous_urine = derive_description_variants("Malodorous urine");
        assert!(malodorous_urine
            .iter()
            .any(|variant| variant.term == "strong smelling"));
        assert!(malodorous_urine
            .iter()
            .any(|variant| variant.term == "smelly"));

        let cold_feet = derive_description_variants("Cold feet");
        assert!(cold_feet
            .iter()
            .any(|variant| variant.term == "feet feel cold"));

        let cold_intolerance = derive_description_variants("Intolerant of cold");
        assert!(cold_intolerance
            .iter()
            .any(|variant| variant.term == "cold all the time"));
        assert!(!cold_intolerance
            .iter()
            .any(|variant| variant.term == "feels cold"));

        let pins_and_needles = derive_description_variants("Pins and needles");
        assert!(pins_and_needles
            .iter()
            .any(|variant| variant.term == "pins needles"));

        let fatigue = derive_description_variants("Fatigue");
        assert!(fatigue.iter().any(|variant| variant.term == "exhausted"));
        assert!(fatigue.iter().any(|variant| variant.term == "no energy"));

        let palpitations = derive_description_variants("Palpitations");
        assert!(palpitations
            .iter()
            .any(|variant| variant.term == "heart races"));

        let swelling = derive_description_variants("Swelling");
        assert!(swelling.iter().any(|variant| variant.term == "swollen"));

        let tenderness = derive_description_variants("Tenderness");
        assert!(tenderness.iter().any(|variant| variant.term == "tender"));

        let erythema = derive_description_variants("Erythema");
        assert!(erythema.iter().any(|variant| variant.term == "red hot"));

        let hot_skin = derive_description_variants("Hot skin");
        assert!(hot_skin.iter().any(|variant| variant.term == "hot swollen"));

        let shiny_skin = derive_description_variants("Shiny skin");
        assert!(shiny_skin
            .iter()
            .any(|variant| variant.term == "skin tight shiny"));
    }

    #[test]
    fn derives_site_discharge_and_context_trim_variants() {
        let discharge = derive_description_variants("Discharge from eye");
        assert!(discharge
            .iter()
            .any(|variant| variant.term == "eye discharge"
                && variant.source == "openehr-description-clinical-phrase-variant"
                && !variant.allow_ambiguous));

        let galactorrhea =
            derive_description_variants("Galactorrhea not associated with childbirth");
        assert!(galactorrhea
            .iter()
            .any(|variant| variant.term == "galactorrhea"
                && variant.source == "openehr-description-clinical-phrase-variant"
                && !variant.allow_ambiguous));
    }

    #[test]
    fn derives_reduced_function_variants_from_impairment_descriptions() {
        let derived = derive_description_variants("Impaired hearing");

        assert!(derived
            .iter()
            .any(|variant| variant.term == "reduced hearing"
                && variant.source == "openehr-description-clinical-phrase-variant"
                && !variant.allow_ambiguous));
    }

    #[test]
    fn derives_low_mood_from_depressed_mood() {
        let derived = derive_description_variants("Depressed mood");

        assert!(derived.iter().any(|variant| variant.term == "low mood"
            && variant.source == "openehr-description-clinical-phrase-variant"
            && !variant.allow_ambiguous));
    }

    #[test]
    fn derives_numeric_initial_labels_from_simple_rate_observables() {
        assert_eq!(
            observable_numeric_labels("Pulse rate"),
            vec!["Pulse".to_string(), "P".to_string()]
        );
        assert_eq!(
            observable_numeric_labels("Respiratory rate"),
            vec!["Respiratory".to_string(), "R".to_string()]
        );
        assert_eq!(
            observable_numeric_labels("BT - Body temperature"),
            vec!["T".to_string()]
        );
        assert!(observable_numeric_labels("Blood pressure").is_empty());
    }
}
