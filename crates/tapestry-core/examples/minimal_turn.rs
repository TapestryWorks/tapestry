//! Minimal end-to-end turn using the mock model client.

use std::sync::Arc;

use tapestry_core::{EchoTool, MockModelClient, TapestryThread, ToolRegistry};
use tapestry_protocol::{EventMsg, Op};

#[tokio::main]
async fn main() {
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(EchoTool));

    let (_thread, handle) = TapestryThread::spawn(Arc::new(MockModelClient), Arc::new(tools));

    let config_id = handle
        .submit(Op::Configure {
            model: "mock-model".into(),
            system_prompt: Some("You are Tapestry.".into()),
        })
        .await
        .expect("configure");

    handle.collect_events_for(config_id).await;

    let turn_id = handle
        .submit(Op::StartTurn {
            content: "hello tapestry".into(),
            name: None,
            turn_id: None,
        })
        .await
        .expect("start turn");

    for event in handle.collect_events_for(turn_id).await {
        match event.msg {
            EventMsg::AgentMessage(msg) => println!("agent: {}", msg.content),
            EventMsg::TurnComplete(done) => {
                println!("turn complete ({:?})", done.finish_reason)
            }
            other => println!("event: {other:?}"),
        }
    }
}
