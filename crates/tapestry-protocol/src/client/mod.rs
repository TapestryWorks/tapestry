//! Client-facing protocol: submissions in, events out.

pub mod event;
pub mod submission;

pub use event::*;
pub use submission::*;
