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
cargo test -p tapestry-core --features genai
```

### Multi-provider models (genai)

The `genai` feature on `tapestry-core` adds a unified adapter for OpenAI, Anthropic,
Gemini, xAI, Groq, DeepSeek, Ollama, and other providers supported by
[`genai`](https://crates.io/crates/genai) 0.6.

Build with the feature:

```bash
cargo build -p tapestry-core --features genai
```

**Model naming** — either form works:

- Simple name (adapter inferred): `gpt-4o-mini`, `claude-sonnet-4-20250514`
- Namespaced: `openai::gpt-4o-mini`, `ollama::llama3.2`, `groq::llama-3.3-70b-versatile`

**Configuration** — genai reads standard provider env vars by default (for example
`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `GROQ_API_KEY`). Local Ollama needs no key;
optionally set `OLLAMA_HOST` (default `http://127.0.0.1:11434`).

Explicit configuration (no hard-coded secrets):

```rust
use tapestry_core::{GenaiChatService, GenaiProviderConfig, GenaiModelClient};

let service = GenaiChatService::from_config(
    &GenaiProviderConfig::new("gpt-4o-mini")
        .with_api_key_env("OPENAI_API_KEY"), // or .with_api_key(...) for tests
)?;
let turn = service.chat_simple(Some("You are helpful."), "Hello!").await?;

// Or plug into the agent loop via ModelClient:
let model = GenaiModelClient::from_model("ollama::llama3.2")?;
```

Run the multi-provider example (skips providers whose env vars are missing):

```bash
cargo run -p tapestry-core --features genai --example genai_multi_provider
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
