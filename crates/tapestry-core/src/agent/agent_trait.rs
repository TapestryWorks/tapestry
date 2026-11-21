use async_trait::async_trait;
use tapestry_protocol::TurnId;

use crate::config::ThreadConfig;
use crate::error::CoreError;
use crate::recorder::Recorder;

/// Executes one agent turn and records its events.
#[async_trait]
pub trait Agent: Send + Sync {
    async fn forward(
        &self,
        recorder: &mut Recorder,
        turn_id: TurnId,
        content: String,
        name: Option<String>,
        config: &ThreadConfig,
    ) -> Result<(), CoreError>;
}
