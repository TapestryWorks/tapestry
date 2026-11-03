use thiserror::Error;

use tapestry_protocol::ProtocolError;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("thread not configured")]
    NotConfigured,
    #[error("turn already in progress")]
    TurnInProgress,
    #[error("no turn in progress")]
    NoTurnInProgress,
    #[error("model error: {0}")]
    Model(String),
    #[error("protocol error: {0}")]
    Protocol(#[from] ProtocolError),
    #[error("channel closed")]
    ChannelClosed,
    #[error("tool error: {0}")]
    Tool(String),
    #[error("internal state error: {0}")]
    Internal(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
}
