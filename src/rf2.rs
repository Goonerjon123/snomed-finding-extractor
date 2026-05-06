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
        let variants = descriptions
            .iter()
            .filter(|description| description.description_type != "fully_specified_name")
            .map(|description| TermVariant {
                text: description.term.clone(),
                source: format!("rf2-{}", description.description_type),
                description_id: Some(description.description_id.clone()),
                allow_ambiguous: false,
            })
            .collect();

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
    if normalize_term(text).is_empty() {
        return;
    }
    if variants
        .iter()
        .any(|variant| normalize_term(&variant.text) == normalize_term(text))
    {
        return;
    }

    variants.push(TermVariant {
        text: text.to_string(),
        source: source.to_string(),
        description_id,
        allow_ambiguous,
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

        if acronym_matches_expansion(prefix, expansion) {
            variants.push(DerivedVariant {
                term: prefix.to_string(),
                source: "openehr-description-acronym",
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

    variants
}

fn split_acronym_expansion(term: &str) -> Option<(&str, &str)> {
    let (prefix, expansion) = term.split_once(" - ")?;
    let prefix = prefix.trim();
    let expansion = expansion.trim();
    if prefix.len() < 3 || prefix.len() > 12 || expansion.len() < 5 {
        return None;
    }
    if !prefix.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return None;
    }
    if !prefix.chars().any(|ch| ch.is_ascii_uppercase()) {
        return None;
    }
    Some((prefix, expansion))
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
}
