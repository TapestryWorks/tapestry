# Tapestry

A composable Rust framework for building cross-platform AI agents and applications.

## Status

This repository contains the first-version agent kernel inspired by [OpenAI Codex](https://github.com/openai/codex) (`codex-core` / `codex-protocol`). See [`docs/architecture.md`](docs/architecture.md) for the Codex ↔ Tapestry module mapping.

## Crates

| Crate | Description |
|-------|-------------|
| [`tapestry-protocol`](crates/tapestry-protocol) | SQ/EQ protocol types (`Submission`, `Op`, `Event`, `EventMsg`) |
| [`tapestry-core`](crates/tapestry-core) | Thread lifecycle, agent loop, model + tool interfaces |

## Quick start

```bash
cargo test --workspace
```

Minimal programmatic usage:

```rust
use std::sync::Arc;
use tapestry_core::{EchoTool, MockModelClient, TapestryThread, ToolRegistry};
use tapestry_protocol::{EventMsg, Op};

#[tokio::main]
async fn main() {
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(EchoTool));

    let (_thread, handle) = TapestryThread::spawn(
        Arc::new(MockModelClient),
        Arc::new(tools),
    );

    let config_id = handle
        .submit(Op::Configure {
            model: "mock-model".into(),
            system_prompt: None,
        })
        .await
        .unwrap();
    handle.collect_events_for(config_id).await;

    let turn_id = handle
        .submit(Op::StartTurn {
            user_message: "hello".into(),
            turn_id: None,
        })
        .await
        .unwrap();

    for event in handle.collect_events_for(turn_id).await {
        if let EventMsg::AgentMessage(msg) = event.msg {
            println!("{}", msg.content);
        }
    }
}
```

## Development

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## License

Apache-2.0 — see [LICENSE](LICENSE).

Architecture reference: [OpenAI Codex](https://github.com/openai/codex) (`codex-rs/core`, `codex-rs/protocol`).
