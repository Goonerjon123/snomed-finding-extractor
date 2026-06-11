use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use snomed_finding_extractor::rf2::{
    build_from_openehr_valueset, build_from_rf2_snapshot, Rf2BuildInput,
};
use snomed_finding_extractor::synthetic::{generate_synthetic_cases, SyntheticCase};
use snomed_finding_extractor::{
    AliasSet, DiagnosisExtractRequest, ExaminationFindingsExtractRequest, ExtractRequest,
    Extractor, ObservableExtractRequest, TerminologyArtefact,
};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Extract candidate findings from a SOAP JSON request.
    Extract {
        #[arg(long)]
        artefact: PathBuf,
        #[arg(long)]
        input: Option<PathBuf>,
        #[arg(long)]
        include_suppressed: bool,
    },

    /// Extract observable entity candidates from an Objective-only JSON request.
    ExtractObservables {
        #[arg(long)]
        artefact: PathBuf,
        #[arg(long)]
        input: Option<PathBuf>,
        #[arg(long)]
        include_suppressed: bool,
    },

    /// Extract examination finding candidates from an Objective-only JSON request.
    ExtractExaminationFindings {
        #[arg(long)]
        artefact: PathBuf,
        #[arg(long)]
        input: Option<PathBuf>,
        #[arg(long)]
        include_suppressed: bool,
    },

    /// Extract diagnosis/disorder candidates from an Assessment-only JSON request.
    ExtractDiagnoses {
        #[arg(long)]
        artefact: PathBuf,
        #[arg(long)]
        input: Option<PathBuf>,
        #[arg(long)]
        include_suppressed: bool,
    },

    /// Build an artefact directly from a snoBehr/openEHR value set manifest.
    BuildOpenehr {
        #[arg(long)]
        valueset: PathBuf,
        #[arg(long)]
        aliases: Option<PathBuf>,
        #[arg(long)]
        output: PathBuf,
    },

    /// Build an artefact from RF2 snapshot files and a refset snapshot.
    BuildRf2 {
        #[arg(long)]
        concept_snapshot: PathBuf,
        #[arg(long)]
        description_snapshot: PathBuf,
        #[arg(long)]
        refset_snapshot: PathBuf,
        #[arg(long)]
        language_snapshot: Option<PathBuf>,
        #[arg(long)]
        refset_id: String,
        #[arg(long)]
        terminology_version: String,
        #[arg(long)]
        source_release: String,
        #[arg(long)]
        aliases: Option<PathBuf>,
        #[arg(long)]
        output: PathBuf,
    },

    /// Report terms the ambiguity guard removed from an artefact.
    AuditTerms {
        #[arg(long)]
        artefact: PathBuf,
    },

    /// Generate synthetic labelled SOAP cases from an artefact.
    GenerateSynthetic {
        #[arg(long)]
        artefact: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value_t = 100)]
        max_concepts: usize,
    },

    /// Evaluate extraction against labelled synthetic/golden cases.
    Evaluate {
        #[arg(long)]
        artefact: PathBuf,
        #[arg(long)]
        cases: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Extract {
            artefact,
            input,
            include_suppressed,
        } => {
            let extractor = load_extractor(&artefact)?;
            let mut request: ExtractRequest = serde_json::from_str(&read_input(input.as_ref())?)
                .context("failed to parse extraction request JSON")?;
            request.include_suppressed |= include_suppressed;
            let response = extractor.extract(request)?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        Command::ExtractObservables {
            artefact,
            input,
            include_suppressed,
        } => {
            let extractor = load_extractor(&artefact)?;
            let mut request: ObservableExtractRequest =
                serde_json::from_str(&read_input(input.as_ref())?)
                    .context("failed to parse observable extraction request JSON")?;
            request.include_suppressed |= include_suppressed;
            let response = extractor.extract_observables(request)?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        Command::ExtractExaminationFindings {
            artefact,
            input,
            include_suppressed,
        } => {
            let extractor = load_extractor(&artefact)?;
            let mut request: ExaminationFindingsExtractRequest =
                serde_json::from_str(&read_input(input.as_ref())?)
                    .context("failed to parse examination findings extraction request JSON")?;
            request.include_suppressed |= include_suppressed;
            let response = extractor.extract_examination_findings(request)?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        Command::ExtractDiagnoses {
            artefact,
            input,
            include_suppressed,
        } => {
            let extractor = load_extractor(&artefact)?;
            let mut request: DiagnosisExtractRequest =
                serde_json::from_str(&read_input(input.as_ref())?)
                    .context("failed to parse diagnosis extraction request JSON")?;
            request.include_suppressed |= include_suppressed;
            let response = extractor.extract_diagnoses(request)?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        Command::BuildOpenehr {
            valueset,
            aliases,
            output,
        } => {
            let mut artefact = build_from_openehr_valueset(valueset)?;
            apply_aliases_if_present(&mut artefact, aliases.as_ref())?;
            artefact.write_pretty_json(output)?;
        }
        Command::BuildRf2 {
            concept_snapshot,
            description_snapshot,
            refset_snapshot,
            language_snapshot,
            refset_id,
            terminology_version,
            source_release,
            aliases,
            output,
        } => {
            let input = Rf2BuildInput {
                concept_snapshot,
                description_snapshot,
                refset_snapshot,
                language_snapshot,
                refset_id,
                terminology_version,
                source_release,
            };
            let mut artefact = build_from_rf2_snapshot(input)?;
            apply_aliases_if_present(&mut artefact, aliases.as_ref())?;
            artefact.write_pretty_json(output)?;
        }
        Command::AuditTerms { artefact } => {
            let extractor = load_extractor(&artefact)?;
            let dropped = extractor.dropped_ambiguous_terms();
            let report = AmbiguityReport {
                refset_id: extractor.artefact().refset_id.clone(),
                terminology_version: extractor.artefact().terminology_version.clone(),
                dropped_ambiguous_count: dropped.len(),
                dropped_ambiguous: dropped.to_vec(),
            };
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Command::GenerateSynthetic {
            artefact,
            output,
            max_concepts,
        } => {
            let artefact = TerminologyArtefact::from_path(artefact)?;
            let cases = generate_synthetic_cases(&artefact, max_concepts);
            serde_json::to_writer_pretty(File::create(output)?, &cases)?;
        }
        Command::Evaluate { artefact, cases } => {
            let extractor = load_extractor(&artefact)?;
            let reader = BufReader::new(File::open(cases)?);
            let cases: Vec<SyntheticCase> = serde_json::from_reader(reader)?;
            let report = evaluate_cases(&extractor, &cases)?;
            println!("{}", serde_json::to_string_pretty(&report)?);

            if report.false_positive_count > 0 || report.false_negative_count > 0 {
                bail!("evaluation failed: safety regressions detected");
            }
        }
    }

    Ok(())
}

fn load_extractor(path: &PathBuf) -> Result<Extractor> {
    let artefact = TerminologyArtefact::from_path(path)?;
    Extractor::new(artefact).context("failed to create extractor")
}

fn read_input(path: Option<&PathBuf>) -> Result<String> {
    let mut buffer = String::new();
    match path {
        Some(path) => File::open(path)
            .with_context(|| format!("failed to open input {}", path.display()))?
            .read_to_string(&mut buffer)?,
        None => io::stdin().read_to_string(&mut buffer)?,
    };
    Ok(buffer)
}

fn apply_aliases_if_present(
    artefact: &mut TerminologyArtefact,
    path: Option<&PathBuf>,
) -> Result<()> {
    if let Some(path) = path {
        let aliases = AliasSet::from_path(path)
            .with_context(|| format!("failed to load alias set {}", path.display()))?;
        artefact.apply_aliases(aliases)?;
    }

    Ok(())
}

#[derive(Debug, serde::Serialize)]
struct AmbiguityReport {
    refset_id: String,
    terminology_version: String,
    dropped_ambiguous_count: usize,
    dropped_ambiguous: Vec<snomed_finding_extractor::DroppedTerm>,
}

#[derive(Debug, serde::Serialize)]
struct EvaluationReport {
    case_count: usize,
    passed_count: usize,
    false_positive_count: usize,
    false_negative_count: usize,
    failures: Vec<EvaluationFailure>,
}

#[derive(Debug, serde::Serialize)]
struct EvaluationFailure {
    case_id: String,
    missing_expected_positive: Vec<String>,
    unexpected_positive: Vec<String>,
    missing_expected_suppressed: Vec<String>,
}

fn evaluate_cases(extractor: &Extractor, cases: &[SyntheticCase]) -> Result<EvaluationReport> {
    let mut failures = Vec::new();

    for case in cases {
        let response = extractor.extract(case.request.clone())?;
        let positives = response
            .matches
            .iter()
            .map(|item| item.concept_id.clone())
            .collect::<BTreeSet<_>>();
        let suppressed = response
            .suppressed
            .iter()
            .map(|item| item.concept_id.clone())
            .collect::<BTreeSet<_>>();
        let expected_positive = case
            .expected_positive_concept_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let expected_suppressed = case
            .expected_suppressed_concept_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();

        let missing_expected_positive = expected_positive
            .difference(&positives)
            .cloned()
            .collect::<Vec<_>>();
        let unexpected_positive = positives
            .difference(&expected_positive)
            .cloned()
            .collect::<Vec<_>>();
        let missing_expected_suppressed = expected_suppressed
            .difference(&suppressed)
            .cloned()
            .collect::<Vec<_>>();

        if !missing_expected_positive.is_empty()
            || !unexpected_positive.is_empty()
            || !missing_expected_suppressed.is_empty()
        {
            failures.push(EvaluationFailure {
                case_id: case.id.clone(),
                missing_expected_positive,
                unexpected_positive,
                missing_expected_suppressed,
            });
        }
    }

    let false_positive_count = failures
        .iter()
        .map(|failure| failure.unexpected_positive.len())
        .sum();
    let false_negative_count = failures
        .iter()
        .map(|failure| {
            failure.missing_expected_positive.len() + failure.missing_expected_suppressed.len()
        })
        .sum();

    Ok(EvaluationReport {
        case_count: cases.len(),
        passed_count: cases.len() - failures.len(),
        false_positive_count,
        false_negative_count,
        failures,
    })
}
