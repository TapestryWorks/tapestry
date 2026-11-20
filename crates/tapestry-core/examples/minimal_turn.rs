//! Minimal end-to-end turn using the mock model client.

use std::sync::Arc;

use tapestry_core::{EchoTool, MockModelClient, TapestryThread, ToolRegistry};
use tapestry_protocol::{Command, EventMsg, ThreadSettings};

#[tokio::main]
async fn main() {
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(EchoTool));

    let (_thread, handle) = TapestryThread::spawn(Arc::new(MockModelClient), Arc::new(tools));

    let config_id = handle
        .submit(Command::UpdateSettings {
            settings: ThreadSettings {
                model: Some("mock-model".into()),
                system_prompt: Some("You are Tapestry.".into()),
                ..Default::default()
            },
        })
        .await
        .expect("configure");

    // Every thread records into an event log; subscribe from the beginning.
    let mut events = handle.subscribe(0).await.expect("subscribe");
    events
        .collect_events_for(config_id)
        .await
        .expect("configured");

    let turn_id = handle
        .submit(Command::StartTurn {
            content: "hello tapestry".into(),
            name: None,
            turn_id: None,
        })
        .await
        .expect("start turn");

    for event in events.collect_events_for(turn_id).await.expect("turn") {
        match event.msg {
            EventMsg::AssistantMessageStart(_) => {
                print!("[{}] streaming: ", event.log_position)
            }
            EventMsg::AssistantMessageDelta(delta) => print!("{}", delta.delta),
            EventMsg::AssistantMessage(msg) => {
                println!();
                if let Some(content) = msg.content {
                    println!("[{}] assistant: {content}", event.log_position);
                }
                for call in msg.tool_calls {
                    println!("[{}] tool call: {}", event.log_position, call.name);
                }
            }
            EventMsg::TurnComplete(done) => {
                println!("turn complete ({:?})", done.finish_reason)
            }
            other => println!("[{}] event: {other:?}", event.log_position),
        }
    }
}
