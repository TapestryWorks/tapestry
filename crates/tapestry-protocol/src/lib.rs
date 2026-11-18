//! Tapestry agent protocol types.
//!
//! Uses a SQ (Submission Queue) / EQ (Event Queue) pattern for asynchronous
//! communication between clients and the agent core, inspired by
//! [OpenAI Codex `codex-protocol`](https://github.com/openai/codex).
//!
//! Every protocol type lives here, grouped by layer. Dependencies point one way:
//! `common` (ids, environment context, system reminders) <- `tool` <- `model`
//! (including `HistoryItem`), while `event` and `submission` build only on
//! `common` and `tool`.

pub mod adapter;
pub mod common;
pub mod error;
pub mod event;
pub mod model;
pub mod submission;
pub mod tool;

pub use adapter::*;
pub use common::*;
pub use error::ProtocolError;
pub use event::*;
pub use model::*;
pub use submission::*;
pub use tool::*;
