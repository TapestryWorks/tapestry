//! Tapestry agent protocol types.
//!
//! Uses a SQ (Submission Queue) / EQ (Event Queue) pattern for asynchronous
//! communication between clients and the agent core, inspired by
//! [OpenAI Codex `codex-protocol`](https://github.com/openai/codex).

pub mod error;
pub mod event;
pub mod ids;
pub mod submission;

pub use error::ProtocolError;
pub use event::*;
pub use ids::*;
pub use submission::*;
