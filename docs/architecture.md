# Tapestry Architecture

Tapestry is a composable Rust framework for building cross-platform AI agents. This document maps the first-version core layout to [OpenAI Codex](https://github.com/openai/codex) (`codex-rs/`), which informed the design.

> **Attribution:** SQ/EQ communication, turn lifecycle, and event taxonomy follow patterns from Codex `codex-protocol` and `codex-core`. Tapestry reimplements them idiomatically for this framework; it is not a fork.

## Workspace layout

| Crate | Codex analogue | Responsibility |
|-------|----------------|----------------|
| `tapestry-protocol` | `codex-protocol` | Submission Queue / Event Queue types (`Submission`, `Op`, `Event`, `EventMsg`), IDs, thread settings |
| `tapestry-core` | `codex-core` | Thread lifecycle, agent loop, model client trait, tool dispatch |

Future crates (not in scope for ALGO-14):

| Planned crate | Codex analogue | Notes |
|---------------|----------------|-------|
| `tapestry-app-server` | `codex-app-server` | JSON-RPC long process for IDE/SDK integration |
| `tapestry-sandbox-*` | sandbox crates | Platform sandboxing and process hardening |
| `tapestry-cli` | `codex-cli` / `codex-tui` | CLI / TUI frontends |

## Communication model

Clients interact with a thread through two async channels:

```text
 Client                              TapestryThread
   |                                        |
   |  Submission (SQ)                       |
   |--------------------------------------->|
   |                                        | process Op
   |  Event (EQ)                            |
   |<---------------------------------------|
```

- **Submission Queue (SQ):** client → agent requests (`Op`)
- **Event Queue (EQ):** agent → client streaming updates (`EventMsg`)

Each `Submission` carries a correlating `SubmissionId`; every `Event` echoes that id so clients can match responses to requests.

## Module mapping (Codex → Tapestry)

| Codex (`codex-core` / `codex-protocol`) | Tapestry | Notes |
|-------------------------------------------|----------|-------|
| `protocol.rs` — `Submission`, `Op`, `Event`, `EventMsg` | `tapestry-protocol::{submission, event}` | Reduced op/event set for v1 |
| `codex_thread.rs` — thread lifecycle | `tapestry-core::thread` | `TapestryThread`, `ThreadHandle` |
| `agent/` — turn control & loop | `tapestry-core::agent` | `AgentLoop::run_turn` |
| `client.rs` — model streaming | `tapestry-core::model` | `ModelClient` trait + `MockModelClient` |
| config / session settings | `tapestry-core::config` + `ThreadSettings` | Sparse overrides |
| tool execution | `tapestry-core::tool` | `Tool` trait + `ToolRegistry` |

## Turn execution flow

```mermaid
sequenceDiagram
    participant Client
    participant Thread as TapestryThread
    participant Loop as AgentLoop
    participant Model as ModelClient
    participant Tools as ToolRegistry

    Client->>Thread: Op::Configure
    Thread-->>Client: SessionConfigured

    Client->>Thread: Op::StartTurn
    Thread-->>Client: TurnStarted
    loop until no tool calls
        Loop->>Model: complete(request)
        Model-->>Loop: content / tool_calls
        alt tool_calls
            Loop->>Tools: execute
            Tools-->>Loop: output
            Loop-->>Client: ToolCallBegin / ToolCallEnd
        else final message
            Loop-->>Client: AgentMessage
        end
    end
    Loop-->>Client: TokenCount
    Loop-->>Client: TurnComplete
```

## v1 protocol surface

### Operations (`Op`)

- `Configure` — initialize thread model + system prompt
- `StartTurn` — run one user turn through the agent loop
- `Interrupt` — abort the in-flight turn
- `ThreadSettings` — apply sparse configuration overrides

### Events (`EventMsg`)

- `SessionConfigured`, `TurnStarted`, `AgentMessage`
- `ToolCallBegin`, `ToolCallEnd`
- `TokenCount`, `TurnComplete`, `TurnAborted`, `Error`

## Testing strategy

- **Protocol:** serde round-trip unit tests for every public request/event type
- **Core:** `MockModelClient` drives deterministic turn paths (echo + tool loop)
- **Integration:** `TapestryThread` end-to-end tests via `ThreadHandle`

Run locally:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
