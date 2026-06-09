//! Explainable SNOMED CT clinical finding extraction for SOAP notes.
//!
//! The crate deliberately keeps runtime behaviour deterministic. A terminology
//! artefact is built offline from licensed RF2/refset inputs, then loaded by the
//! matcher at runtime. The extractor returns candidate findings for clinician
//! confirmation and can include suppressed evidence for audit and validation.

pub mod context;
pub mod error;
pub mod extractor;
pub mod matcher;
pub mod model;
pub mod normalization;
pub mod rf2;
pub mod synthetic;
pub mod terminology;

pub use crate::error::{ExtractorError, Result};
pub use crate::extractor::Extractor;
pub use crate::model::{
    AssertionStatus, DiagnosisExtractRequest, ExaminationFindingsExtractRequest, ExtractRequest,
    ExtractResponse, FindingMatch, ObservableExtractRequest, SoapField, SuppressedMatch,
};
pub use crate::terminology::{AliasSet, TerminologyArtefact};

pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const RULESET_VERSION: &str = "ruleset-2026-06-09-v4";
