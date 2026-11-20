use std::sync::Arc;

use tapestry_adapter_agui::{AguiAdapter, AguiEvent};
use tapestry_core::{EchoTool, MockModelClient, TapestryThread, ToolRegistry};
use tapestry_protocol::{Command, EventAdapter, ThreadSettings};

#[tokio::test]
async fn mock_tool_turn_maps_the_complete_event_log_to_agui() {
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(EchoTool));
    let (_thread, handle) = TapestryThread::spawn(Arc::new(MockModelClient), Arc::new(tools));
    let mut subscription = handle.subscribe(0).await.unwrap();

    let configured = handle
        .submit(Command::UpdateSettings {
            settings: ThreadSettings {
                model: Some("mock-model".into()),
                include_time_context: Some(false),
                ..Default::default()
            },
        })
        .await
        .unwrap();
    subscription.collect_events_for(configured).await.unwrap();

    let turn = handle
        .submit(Command::StartTurn {
            content: "please tool:echo now".into(),
            name: None,
            turn_id: None,
        })
        .await
        .unwrap();
    let log = subscription.collect_events_for(turn).await.unwrap();
    let mut adapter = AguiAdapter::default();
    let output: Vec<_> = log.iter().flat_map(|event| adapter.adapt(event)).collect();
    let types: Vec<_> = output
        .iter()
        .map(|event| match event {
            AguiEvent::RunStarted { .. } => "RUN_STARTED",
            AguiEvent::RunFinished { .. } => "RUN_FINISHED",
            AguiEvent::StepStarted { .. } => "STEP_STARTED",
            AguiEvent::StepFinished { .. } => "STEP_FINISHED",
            AguiEvent::TextMessageStart { .. } => "TEXT_MESSAGE_START",
            AguiEvent::TextMessageContent { .. } => "TEXT_MESSAGE_CONTENT",
            AguiEvent::TextMessageEnd { .. } => "TEXT_MESSAGE_END",
            AguiEvent::ToolCallStart { .. } => "TOOL_CALL_START",
            AguiEvent::ToolCallArgs { .. } => "TOOL_CALL_ARGS",
            AguiEvent::ToolCallEnd { .. } => "TOOL_CALL_END",
            AguiEvent::ToolCallResult { .. } => "TOOL_CALL_RESULT",
            AguiEvent::RunError { .. } => "RUN_ERROR",
            AguiEvent::Custom { .. } => "CUSTOM",
        })
        .collect();
    assert_eq!(
        types.first(),
        Some(&"RUN_STARTED"),
        "first event: {types:?}"
    );
    assert_eq!(
        types.iter().filter(|kind| **kind == "STEP_STARTED").count(),
        2
    );
    assert_eq!(
        types
            .iter()
            .filter(|kind| **kind == "STEP_FINISHED")
            .count(),
        2
    );
    assert!(types.windows(4).any(|window| {
        window
            == [
                "TOOL_CALL_START",
                "TOOL_CALL_ARGS",
                "TOOL_CALL_END",
                "TOOL_CALL_RESULT",
            ]
    }));
    let text_start = types
        .iter()
        .position(|kind| *kind == "TEXT_MESSAGE_START")
        .unwrap();
    let text_end = types
        .iter()
        .position(|kind| *kind == "TEXT_MESSAGE_END")
        .unwrap();
    assert!(text_start < text_end);
    assert!(types[text_start + 1..text_end]
        .iter()
        .all(|kind| *kind == "TEXT_MESSAGE_CONTENT"));
    assert!(text_end > text_start + 1);
    assert_eq!(types.last(), Some(&"RUN_FINISHED"));
}
