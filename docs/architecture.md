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
| `agent/` — turn control & loop | `tapestry-core::{turn, agent}` | `TurnMachine` (pure decisions) + `AgentLoop::run_turn` (IO driver) |
| conversation history | `tapestry-core::history` | `HistoryItem`, `render_user_content` |
| `client.rs` — model streaming | `tapestry-core::model` | `ModelClient` trait + `MockModelClient` |
| config / session settings | `tapestry-core::config` + `ThreadSettings` | Sparse overrides |
| tool execution | `tapestry-core::tool` | `Tool` trait, `ToolSpec`, `ToolRegistry` |

## Turn execution

### State machine and driver

`TurnMachine` is synchronous and performs no IO. It exposes `next_step()` (`CallModel`, `RunTools`, `Done`) and is advanced by `on_model_output` / `on_tool_outcomes`, which also write the turn's items to history. `AgentLoop::run_turn` is the async driver: it performs the step, times tools, and emits events. `RunTools` is a batch so parallel execution can be added later.

- A model output without tool calls ends the turn with `finish_reason: completed`.
- Tool calls are executed in order and their results fed back; unknown tools (`not_found`) and failures (`failed`) are results, not errors, and the loop continues.
- After `max_tool_iterations` model calls that still requested tools, the machine runs those tools, then makes one more call with no tools and a temporary reminder (not stored in history). The turn ends with `finish_reason: exceed_max_iters`; tool calls in that final output are ignored and a `Warning` is emitted.
- On model failure or a closed event channel the turn returns an error (the thread emits `Error`), and every unanswered tool call in history receives a `failed` result so later requests stay valid.

### History and names

The thread session owns a `Vec<HistoryItem>` (`User{name}`, `Assistant{content, tool_calls}`, `ToolResult`) that persists across turns. `ModelRequest` carries `system_prompt`, `history`, `tools` (empty when tools are disallowed) and `iteration`.

- `Op::StartTurn.user_name` becomes `User.name`; it must be non-empty and contain no line breaks, otherwise the thread emits `Error` and history is unchanged.
- Adapters render user messages with `render_user_content` as `"{name}: {content}"` (a text prefix, not a provider `name` field); consecutive user messages are not merged.
- Assistant items never carry a name. The agent's own name (`ThreadSettings.agent_name`) is appended to the system prompt as `Your name is {name}.`
- Known limit: user content can imitate the `name:` prefix.

### Event order

```text
TurnStarted
  ModelRequestStarted(i)
    [AgentMessage]
    [Warning]
    [ToolCallBegin, ToolCallEnd]*   -- paired per call, in call order
  ModelRequestCompleted(i)
  ... repeated for each model call ...
TokenCount
TurnComplete { finish_reason, last_agent_message }
```

All events carry the triggering `SubmissionId`. `ToolCallEnd` carries `status` (`completed`, `failed`, `not_found`) and `duration_ms`.

```mermaid
sequenceDiagram
    participant Client
    participant Thread as TapestryThread
    participant Loop as AgentLoop
    participant Machine as TurnMachine
    participant Model as ModelClient
    participant Tools as ToolRegistry

    Client->>Thread: Op::Configure
    Thread-->>Client: SessionConfigured

    Client->>Thread: Op::StartTurn
    Thread-->>Client: TurnStarted
    loop until Done
        Loop->>Machine: next_step()
        Loop->>Model: complete(history, tools)
        Loop->>Machine: on_model_output
        opt tool calls
            Loop->>Tools: execute
            Loop-->>Client: ToolCallBegin / ToolCallEnd
            Loop->>Machine: on_tool_outcomes
        end
    end
    Loop-->>Client: TokenCount
    Loop-->>Client: TurnComplete
```

### Current limits and next phases

- Turns are serial: `Interrupt` is queued and only handled after the running turn ends; in-turn cancellation is a later phase.
- History is in memory and unbounded (no compaction).
- Not yet: streaming output, parallel tools, hooks, approvals, multi-agent messaging.

## v1 protocol surface

### Operations (`Op`)

- `Configure` — initialize thread model + system prompt
- `StartTurn` — run one user turn (optional `user_name`)
- `Interrupt` — abort the in-flight turn (`TurnAborted` with reason `interrupted`)
- `ThreadSettings` — apply sparse overrides (`model`, `system_prompt`, `max_tool_iterations`, `agent_name`)

### Events (`EventMsg`)

- `SessionConfigured`, `TurnStarted`, `AgentMessage` (one complete assistant message)
- `ModelRequestStarted`, `ModelRequestCompleted`, `Warning`
- `ToolCallBegin`, `ToolCallEnd`
- `TokenCount`, `TurnComplete`, `TurnAborted`, `Error`

## Cross-platform rules

- `tapestry-protocol` and `tapestry-core` must not depend on crates that build on only one OS or download platform binaries at build time; isolate such code behind a feature or separate crate.
- `tapestry-protocol` must build for `wasm32-unknown-unknown`; `uuid` uses its `js` feature on that target for randomness (`getrandom` version follows `uuid`).
- CI checks the workspace (and `tapestry-core --features genai`) on Linux, macOS and Windows, checks `tapestry-protocol` on wasm32, and keeps fmt, clippy and tests on Linux.

## Testing strategy

- **Protocol:** serde round-trip unit tests for every public request/event type
- **Core:** `TurnMachine` is unit-tested without IO; `AgentLoop` tests assert the event-order contract with scripted models; `MockModelClient` drives deterministic paths (echo + tool loop)
- **Integration:** `TapestryThread` end-to-end tests via `ThreadHandle`

Run locally:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
