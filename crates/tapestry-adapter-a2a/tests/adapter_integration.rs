use std::sync::Arc;

use tapestry_adapter_a2a::{A2aAdapter, StreamResponse, TaskState};
use tapestry_core::{EchoTool, MockModelClient, TapestryThread, ToolRegistry};
use tapestry_protocol::{Command, EventAdapter, ThreadSettings};

#[tokio::test]
async fn mock_tool_turn_maps_the_complete_event_log_to_a2a() {
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
    let mut adapter = A2aAdapter::new();
    let output: Vec<_> = log.iter().flat_map(|event| adapter.adapt(event)).collect();

    assert!(matches!(output.first(), Some(StreamResponse::Task(_))));
    assert!(matches!(
        output.last(),
        Some(StreamResponse::StatusUpdate(status))
            if status.status.state == TaskState::Completed
    ));
    assert!(output[1..output.len() - 1]
        .iter()
        .all(|item| matches!(item, StreamResponse::ArtifactUpdate(_))));
    let artifacts: Vec<_> = output[1..output.len() - 1]
        .iter()
        .map(|item| match item {
            StreamResponse::ArtifactUpdate(update) => update,
            _ => unreachable!(),
        })
        .collect();
    assert!(artifacts.len() >= 2);
    assert!(!artifacts[0].append);
    assert!(artifacts[1..artifacts.len() - 1]
        .iter()
        .all(|update| update.append));
    assert!(!artifacts.last().unwrap().append);
    assert!(artifacts.last().unwrap().last_chunk);
    assert!(output
        .iter()
        .any(|item| matches!(item, StreamResponse::ArtifactUpdate(update)
            if update.last_chunk && update.artifact.parts.iter().any(|part| part.text.contains("echo=")))));
}
