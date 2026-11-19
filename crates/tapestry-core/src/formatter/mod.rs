//! Format internal events and structured context for model inputs.

pub mod formatter_trait;
pub mod prompt;

pub mod openai_formatter;

pub use formatter_trait::Formatter;
pub use formatter_trait::HistoryProjection;
pub use openai_formatter::OpenAiFormatter;
