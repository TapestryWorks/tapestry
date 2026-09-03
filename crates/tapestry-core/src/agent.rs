use std::sync::Arc;

use tapestry_protocol::{
    AgentMessageEvent, Event, EventMsg, SubmissionId, TokenCountEvent, ToolCallBeginEvent,
    ToolCallEndEvent, TurnCompleteEvent, TurnId, TurnStartedEvent,
};
use tokio::sync::mpsc;

use crate::config::ThreadConfig;
use crate::error::CoreError;
use crate::model::{ModelClient, ModelRequest};
use crate::tool::{ToolContext, ToolRegistry};

/// Executes the model → tool → model loop for a single turn.
pub struct AgentLoop {
    model: Arc<dyn ModelClient>,
    tools: Arc<ToolRegistry>,
}

impl AgentLoop {
    pub fn new(model: Arc<dyn ModelClient>, tools: Arc<ToolRegistry>) -> Self {
        Self { model, tools }
    }

    pub async fn run_turn(
        &self,
        submission_id: SubmissionId,
        turn_id: TurnId,
        user_message: String,
        config: &ThreadConfig,
        event_tx: &mpsc::Sender<Event>,
    ) -> Result<(), CoreError> {
        send_event(
            event_tx,
            Event::new(
                submission_id,
                EventMsg::TurnStarted(TurnStartedEvent { turn_id }),
            ),
        )
        .await?;

        let mut tool_results = Vec::new();
        let mut total_input = 0u32;
        let mut total_output = 0u32;
        let mut final_message = None;

        for iteration in 0..config.max_tool_iterations {
            let response = self
                .model
                .complete(ModelRequest {
                    system_prompt: config.system_prompt.clone(),
                    user_message: user_message.clone(),
                    tool_results: tool_results.clone(),
                    iteration,
                })
                .await?;

            total_input += response.input_tokens;
            total_output += response.output_tokens;

            if let Some(content) = response.content.clone() {
                send_event(
                    event_tx,
                    Event::new(
                        submission_id,
                        EventMsg::AgentMessage(AgentMessageEvent {
                            turn_id,
                            content: content.clone(),
                        }),
                    ),
                )
                .await?;
                final_message = Some(content);
            }

            if response.tool_calls.is_empty() {
                break;
            }

            for call in response.tool_calls {
                let tool_call_id = tapestry_protocol::ToolCallId::new();
                send_event(
                    event_tx,
                    Event::new(
                        submission_id,
                        EventMsg::ToolCallBegin(ToolCallBeginEvent {
                            turn_id,
                            tool_call_id,
                            tool_name: call.name.clone(),
                            arguments: call.arguments.clone(),
                        }),
                    ),
                )
                .await?;

                let tool = self
                    .tools
                    .get(&call.name)
                    .ok_or_else(|| CoreError::Tool(format!("unknown tool: {}", call.name)))?;

                let ctx = ToolContext {
                    turn_user_message: user_message.clone(),
                };
                let result = tool.execute(call.arguments.clone(), &ctx).await?;

                send_event(
                    event_tx,
                    Event::new(
                        submission_id,
                        EventMsg::ToolCallEnd(ToolCallEndEvent {
                            turn_id,
                            tool_call_id,
                            tool_name: call.name.clone(),
                            output: result.output.clone(),
                            success: result.success,
                        }),
                    ),
                )
                .await?;

                tool_results.push((call.name, result.output));
            }

            if iteration + 1 == config.max_tool_iterations && final_message.is_none() {
                return Err(CoreError::MaxToolIterations(config.max_tool_iterations));
            }
        }

        send_event(
            event_tx,
            Event::new(
                submission_id,
                EventMsg::TokenCount(TokenCountEvent {
                    turn_id,
                    input_tokens: total_input,
                    output_tokens: total_output,
                    total_tokens: total_input + total_output,
                }),
            ),
        )
        .await?;

        send_event(
            event_tx,
            Event::new(
                submission_id,
                EventMsg::TurnComplete(TurnCompleteEvent { turn_id }),
            ),
        )
        .await?;

        Ok(())
    }
}

async fn send_event(event_tx: &mpsc::Sender<Event>, event: Event) -> Result<(), CoreError> {
    event_tx
        .send(event)
        .await
        .map_err(|_| CoreError::ChannelClosed)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tapestry_protocol::{EventMsg, SubmissionId, TurnId};

    use super::*;
    use crate::model::MockModelClient;
    use crate::tool::{EchoTool, ToolRegistry};

    #[tokio::test]
    async fn agent_loop_echo_turn() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        let loop_ = AgentLoop::new(Arc::new(MockModelClient), Arc::new(registry));

        let (tx, mut rx) = mpsc::channel(16);
        loop_
            .run_turn(
                SubmissionId::new(),
                TurnId::new(),
                "hello".into(),
                &ThreadConfig::default(),
                &tx,
            )
            .await
            .unwrap();
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        assert!(events
            .iter()
            .any(|e| matches!(e.msg, EventMsg::TurnStarted(_))));
        assert!(events
            .iter()
            .any(|e| matches!(e.msg, EventMsg::AgentMessage(_))));
        assert!(events
            .iter()
            .any(|e| matches!(e.msg, EventMsg::TurnComplete(_))));
    }

    #[tokio::test]
    async fn agent_loop_executes_tool_path() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        let loop_ = AgentLoop::new(Arc::new(MockModelClient), Arc::new(registry));

        let (tx, mut rx) = mpsc::channel(16);
        loop_
            .run_turn(
                SubmissionId::new(),
                TurnId::new(),
                "please tool:echo now".into(),
                &ThreadConfig::default(),
                &tx,
            )
            .await
            .unwrap();
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        assert!(events
            .iter()
            .any(|e| matches!(e.msg, EventMsg::ToolCallBegin(_))));
        assert!(events
            .iter()
            .any(|e| matches!(e.msg, EventMsg::ToolCallEnd(_))));
    }
}
