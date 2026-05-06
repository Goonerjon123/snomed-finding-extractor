use crate::error::{ExtractorError, Result};
use crate::normalization::normalize_term;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminologyArtefact {
    pub schema_version: u32,
    pub terminology_version: String,
    pub source_release: String,
    pub refset_id: String,
    pub generated_at_utc: String,
    pub concepts: Vec<ConceptEntry>,
    #[serde(default)]
    pub artefact_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConceptEntry {
    pub concept_id: String,
    pub active: bool,
    pub preferred_term: String,
    #[serde(default)]
    pub descriptions: Vec<DescriptionEntry>,
    #[serde(default)]
    pub variants: Vec<TermVariant>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DescriptionEntry {
    pub description_id: String,
    pub term: String,
    pub description_type: String,
    #[serde(default)]
    pub acceptability: Option<String>,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct TermVariant {
    pub text: String,
    pub source: String,
    #[serde(default)]
    pub description_id: Option<String>,
    #[serde(default)]
    pub allow_ambiguous: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AliasSet {
    pub schema_version: u32,
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<AliasEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AliasEntry {
    pub concept_id: String,
    #[serde(default)]
    pub expected_preferred_term: Option<String>,
    #[serde(default)]
    pub terms: Vec<String>,
    #[serde(default = "default_alias_source")]
    pub source: String,
    #[serde(default)]
    pub allow_ambiguous: bool,
}

impl AliasSet {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let bytes = fs::read(path)?;
        Ok(serde_json::from_slice(strip_utf8_bom(&bytes))?)
    }
}

impl TerminologyArtefact {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let bytes = fs::read(path)?;
        let mut artefact: Self = serde_json::from_slice(strip_utf8_bom(&bytes))?;
        artefact.verify_or_fill_hash()?;
        Ok(artefact)
    }

    pub fn write_pretty_json(&mut self, path: impl AsRef<Path>) -> Result<()> {
        self.artefact_hash = self.compute_hash()?;
        let writer = BufWriter::new(File::create(path)?);
        serde_json::to_writer_pretty(writer, self)?;
        Ok(())
    }

    pub fn verify_or_fill_hash(&mut self) -> Result<()> {
        let computed = self.compute_hash()?;
        if self.artefact_hash.is_empty() || self.artefact_hash == "UNVERIFIED" {
            self.artefact_hash = computed;
            return Ok(());
        }

        if self.artefact_hash != computed {
            return Err(ExtractorError::ArtefactHashMismatch {
                expected: self.artefact_hash.clone(),
                computed,
            });
        }

        Ok(())
    }

    pub fn compute_hash(&self) -> Result<String> {
        let mut clone = self.clone();
        clone.artefact_hash.clear();
        let bytes = serde_json::to_vec(&clone)?;
        let digest = Sha256::digest(bytes);
        Ok(format!("sha256:{}", hex::encode(digest)))
    }

    pub fn validate_runtime_terms(&self) -> Result<()> {
        let mut count = 0_usize;
        for concept in self.concepts.iter().filter(|concept| concept.active) {
            for variant in concept.runtime_variants() {
                if is_blocked_common_term(&normalize_term(&variant.text), variant.allow_ambiguous) {
                    continue;
                }
                count += 1;
            }
        }

        if count == 0 {
            return Err(ExtractorError::EmptyTerminology);
        }

        Ok(())
    }

    pub fn apply_aliases(&mut self, aliases: AliasSet) -> Result<()> {
        for alias in aliases.aliases {
            if alias.terms.is_empty() {
                continue;
            }

            let Some(concept) = self
                .concepts
                .iter_mut()
                .find(|concept| concept.concept_id == alias.concept_id)
            else {
                return Err(ExtractorError::InvalidInput(format!(
                    "alias set {} references concept {} which is not in this artefact",
                    aliases.name, alias.concept_id
                )));
            };

            if let Some(expected) = alias.expected_preferred_term.as_deref() {
                if concept.preferred_term != expected {
                    return Err(ExtractorError::InvalidInput(format!(
                        "alias set {} expected concept {} preferred term {}, found {}",
                        aliases.name, alias.concept_id, expected, concept.preferred_term
                    )));
                }
            }

            for term in alias.terms {
                if normalize_term(&term).is_empty() {
                    continue;
                }
                if concept
                    .variants
                    .iter()
                    .any(|variant| normalize_term(&variant.text) == normalize_term(&term))
                {
                    continue;
                }

                concept.variants.push(TermVariant {
                    text: term,
                    source: format!("clinical_alias:{}", alias.source),
                    description_id: None,
                    allow_ambiguous: alias.allow_ambiguous,
                });
            }
        }

        self.artefact_hash = self.compute_hash()?;
        Ok(())
    }
}

impl ConceptEntry {
    pub fn runtime_variants(&self) -> Vec<TermVariant> {
        let mut variants = self.variants.clone();
        if !self.preferred_term.trim().is_empty() {
            variants.push(TermVariant {
                text: self.preferred_term.clone(),
                source: "preferred_term".to_string(),
                description_id: None,
                allow_ambiguous: false,
            });
        }

        let mut seen = HashSet::new();
        variants
            .into_iter()
            .filter(|variant| seen.insert(normalize_term(&variant.text)))
            .collect()
    }
}

pub fn is_blocked_common_term(normalized: &str, allow_ambiguous: bool) -> bool {
    if allow_ambiguous {
        return false;
    }

    if normalized.chars().filter(|ch| ch.is_alphanumeric()).count() < 3 {
        return true;
    }

    if starts_with_context_only_word(normalized) {
        return true;
    }

    matches!(
        normalized,
        "mi" | "ms" | "ra" | "dm" | "pt" | "ca" | "sob" | "sad" | "fit" | "cold" | "hot"
    )
}

fn starts_with_context_only_word(normalized: &str) -> bool {
    let Some(first_word) = normalized.split(' ').next() else {
        return false;
    };

    matches!(
        first_word,
        "at" | "on"
            | "after"
            | "before"
            | "during"
            | "while"
            | "when"
            | "with"
            | "without"
            | "in"
            | "by"
            | "for"
            | "to"
            | "of"
    )
}

fn strip_utf8_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes)
}

fn default_alias_source() -> String {
    "local-curated".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_aliases_to_existing_concepts() {
        let mut artefact = TerminologyArtefact {
            schema_version: 1,
            terminology_version: "fixture".to_string(),
            source_release: "fixture".to_string(),
            refset_id: "fixture".to_string(),
            generated_at_utc: "fixture".to_string(),
            concepts: vec![ConceptEntry {
                concept_id: "1000000006".to_string(),
                active: true,
                preferred_term: "Dyspnea".to_string(),
                descriptions: vec![],
                variants: vec![],
            }],
            artefact_hash: String::new(),
        };

        artefact
            .apply_aliases(AliasSet {
                schema_version: 1,
                name: "test".to_string(),
                aliases: vec![AliasEntry {
                    concept_id: "1000000006".to_string(),
                    expected_preferred_term: Some("Dyspnea".to_string()),
                    terms: vec!["short of breath".to_string(), "SOB".to_string()],
                    source: "gp-test".to_string(),
                    allow_ambiguous: true,
                }],
            })
            .unwrap();

        assert!(artefact.concepts[0]
            .variants
            .iter()
            .any(|variant| variant.text == "short of breath"
                && variant.source == "clinical_alias:gp-test"));
        assert!(artefact.artefact_hash.starts_with("sha256:"));
    }

    #[test]
    fn blocks_context_fragments_as_standalone_terms() {
        assert!(is_blocked_common_term("at rest", false));
        assert!(is_blocked_common_term("on exertion", false));
        assert!(!is_blocked_common_term("short of breath", false));
    }
}
