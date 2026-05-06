use thiserror::Error;

pub type Result<T> = std::result::Result<T, ExtractorError>;

#[derive(Debug, Error)]
pub enum ExtractorError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("CSV/RF2 error: {0}")]
    Csv(#[from] csv::Error),

    #[error("terminology artefact hash mismatch: expected {expected}, computed {computed}")]
    ArtefactHashMismatch { expected: String, computed: String },

    #[error("terminology artefact contains no usable terms")]
    EmptyTerminology,

    #[error("requested refset {requested} does not match loaded artefact refset {loaded}")]
    RefsetMismatch { requested: String, loaded: String },

    #[error("matcher build failed: {0}")]
    Matcher(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),
}
