use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("invalid submission id: {0}")]
    InvalidSubmissionId(String),
    #[error("serialization failed: {0}")]
    Serialization(String),
}
