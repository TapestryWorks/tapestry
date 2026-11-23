# Tapestry

A composable Rust framework for building cross-platform AI agents and applications.

## Status

This repository contains the first-version agent kernel inspired by [OpenAI Codex](https://github.com/openai/codex) (`codex-core` / `codex-protocol`). See [`docs/architecture.md`](docs/architecture.md) for the Codex ↔ Tapestry module mapping.

## Crates

| Crate | Description |
|-------|-------------|
| [`tapestry-protocol`](crates/tapestry-protocol) | SQ/EQ protocol types (`Submission`, `Command`, `Event`, `EventMsg`) |
| [`tapestry-core`](crates/tapestry-core) | Thread lifecycle, agent loop, model + tool interfaces |
| [`tapestry-adapter-agui`](crates/tapestry-adapter-agui) | Tapestry event stream to AG-UI 1.0 events |
| [`tapestry-adapter-a2a`](crates/tapestry-adapter-a2a) | Tapestry event stream to A2A 1.0 streaming responses |
| [`tapestry-cli`](crates/tapestry-cli) | Run one agent turn and stream its events in the terminal |

## Git pre-commit formatting hook

Install the repository's local Git hook once per clone:

```bash
bash scripts/install-hooks.sh
```

After installation, `git commit` automatically runs `cargo fmt --all` when
staged Rust files are present, re-stages those formatted files, and verifies
`cargo fmt --all -- --check`. No separate formatting command is needed.

**Safety:** If any tracked Rust file has unstaged edits, the hook stops before
formatting; stage or stash those edits first. If formatting changes a Rust file
that was not already staged, the hook stops and asks you to review and stage it
explicitly. Non-Rust-only commits skip formatting. The hook requires Cargo.
To disable the repository hook, run `git config --local --unset core.hooksPath`
(note that doing so also disables any other hooks in this directory).

## Quick start

```bash
cargo test --workspace
cargo test -p tapestry-core
```

### Run an agent from the command line

Create `.tapestry/config.yaml` in your working directory or home directory
with your OpenAI-compatible Chat Completions settings. Use the API base URL,
such as `https://api.openai.com/v1`, and keep your API key private.

```yaml
base_url: https://api.openai.com/v1
api_key: "replace-with-your-api-key"
model: gpt-4o-mini
```

Optionally, create `.tapestry/agents/reviewer.yaml` or `.agents/reviewer.yaml`
with the system prompt for the `reviewer` agent. Without `-a`, the CLI uses the
built-in system prompt `You are a helpful assistant.`

```yaml
system_prompt: |
  You are a code reviewer. Describe any correctness issues you find.
```

The CLI checks configuration files in this order: working-directory
`.tapestry`, working-directory `.agents`, home-directory `.tapestry`, then
home-directory `.agents`. Model settings use `config.yaml` in either directory;
agent settings use `agents/<name>.yaml` in `.tapestry` and `<name>.yaml` in
`.agents`. Agent names use ASCII letters, digits, hyphens, and underscores.

```bash
cargo run -p tapestry-cli -- -a reviewer -p "What should I check in a code review?"
cargo run -p tapestry-cli -- -p "Hello"
```

From another project, install the binary with `cargo install --path crates/tapestry-cli`
from this repository, then use `tapestry-cli -a reviewer -p "Your instruction"`
from that project's directory. Each run prints tool calls
(`tool call: name(args)`), tool results (`tool result: name: output`) and the
assistant's final text to stdout. Failures produce a nonzero exit status and a
diagnostic on stderr.

### Multi-provider models (genai)

`tapestry-core` includes a unified genai adapter for OpenAI, Anthropic, Gemini,
xAI, Groq, DeepSeek, Ollama, and other providers supported by
[`genai`](https://crates.io/crates/genai) 0.6.

Build:

```bash
cargo build -p tapestry-core
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

For explicit OpenAI endpoints, use `OpenAiChatModel::from_model("gpt-4o-mini")`
for Chat Completions, or `OpenAiResponseModel::from_model("gpt-5-mini")` for the
Responses API. Both support streaming and use `OPENAI_API_KEY` by default.
`OpenAiResponseModel` sends a local event snapshot on every call rather than
chaining server-side response IDs. `ModelRequest.events` is the ordered log
snapshot; model clients turn it into provider messages with their formatter.

Formatter behavior can be selected when creating either OpenAI model:

```rust
use std::sync::Arc;
use tapestry_core::{CoreError, OpenAiChatModel, OpenAiFormatter};

fn build_model() -> Result<OpenAiChatModel, CoreError> {
    Ok(OpenAiChatModel::from_model("gpt-4o-mini")?
        .with_formatter(Arc::new(OpenAiFormatter)))
}
```

Direct model clients also construct `ModelRequest` with `events: Vec<Event>`
rather than a pre-projected `history`; each request owns a stable copy of the
thread log up to the call:

```rust
use tapestry_protocol::{Event, ModelRequest};

fn make_request(events: Vec<Event>) -> ModelRequest {
    ModelRequest {
        system_prompt: None,
        events,
        tools: Vec::new(),
        iteration: 0,
    }
}
```

Run the multi-provider example (skips providers whose env vars are missing):

```bash
cargo run -p tapestry-core --example genai_multi_provider
```

Minimal programmatic usage:

```rust
use std::sync::Arc;
use tapestry_core::{EchoTool, MockModelClient, TapestryThread, ToolRegistry};
use tapestry_protocol::{Command, EventMsg, ThreadSettings};

#[tokio::main]
async fn main() {
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(EchoTool));

    let (_thread, handle) = TapestryThread::spawn(
        Arc::new(MockModelClient),
        Arc::new(tools),
    );

    let config_id = handle
        .submit(Command::UpdateSettings {
            settings: ThreadSettings {
                model: Some("mock-model".into()),
                ..Default::default()
            },
        })
        .await
        .unwrap();
    // Read the thread's event log from the beginning.
    let mut events = handle.subscribe(0).await.unwrap();
    events.collect_events_for(config_id).await.unwrap();

    let turn_id = handle
        .submit(Command::StartTurn {
            content: "hello".into(),
            name: None,
            turn_id: None,
        })
        .await
        .unwrap();

    for event in events.collect_events_for(turn_id).await.unwrap() {
        if let EventMsg::AssistantMessage(msg) = event.msg {
            println!("{}", msg.content.unwrap_or_default());
        }
    }
}
```

### Protocol adapters

The adapter crates consume ordered events from `EventSubscription` and convert each
event into zero or more protocol outputs. Call `finish()` when the subscription
ends so an adapter can close incomplete messages or tasks. `EventFilter` lets
consumers opt into categories emitted as AG-UI `CUSTOM`; the A2A adapter ignores
events without a task-level mapping.

```rust
use std::error::Error;

use tapestry_adapter_agui::AguiAdapter;
use tapestry_core::ThreadHandle;
use tapestry_protocol::{EventAdapter, EventFilter, SubmissionId};

async fn consume(
    thread_handle: &ThreadHandle,
    turn_id: SubmissionId,
) -> Result<(), Box<dyn Error>> {
    let mut adapter = AguiAdapter::new(EventFilter::none());
    let mut events = thread_handle.subscribe(0).await?;
    for event in events.collect_events_for(turn_id).await? {
        for output in adapter.adapt(&event) {
            println!("{}", serde_json::to_string(&output)?);
        }
    }
    for output in adapter.finish() {
        println!("{}", serde_json::to_string(&output)?);
    }
    Ok(())
}
```

When adding an `EventMsg` variant, assign its `EventCategory` and update each
protocol adapter's mapping and tests.

## Development

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## License

Apache-2.0 — see [LICENSE](LICENSE).

Architecture reference: [OpenAI Codex](https://github.com/openai/codex) (`codex-rs/core`, `codex-rs/protocol`).
