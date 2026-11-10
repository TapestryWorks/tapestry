//! Tapestry agent protocol types.
//!
//! Uses a SQ (Submission Queue) / EQ (Event Queue) pattern for asynchronous
//! communication between clients and the agent core, inspired by
//! [OpenAI Codex `codex-protocol`](https://github.com/openai/codex).
//!
//! Every protocol type lives here, grouped by layer. Dependencies point one way:
//! `ids` <- `common` <- `tool` <- `history` <- {`model`, `react`}, while `client`
//! builds on `ids`, `common` and `tool` and never on `react`.

pub mod client;
pub mod common;
pub mod error;
pub mod history;
pub mod ids;
pub mod model;
pub mod react;
pub mod tool;

pub use client::*;
pub use common::*;
pub use error::ProtocolError;
pub use history::*;
pub use ids::*;
pub use model::*;
pub use react::*;
pub use tool::*;
