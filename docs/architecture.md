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
| `protocol.rs` — `Submission`, `Op`, `Event`, `EventMsg` | `tapestry-protocol::client::{submission, event}` | Reduced op/event set for v1 |
| `codex_thread.rs` — thread lifecycle | `tapestry-core::thread` | `TapestryThread`, `ThreadHandle` |
| `agent/` — turn control & loop | `tapestry-core::{turn, agent}` | `TurnMachine` (pure decisions) + `AgentLoop::run_turn` (IO driver) |
| conversation history | `tapestry-protocol::history` (types), `tapestry-core::history` (helpers) | `HistoryItem`, `EnvironmentContext`, `SystemReminder`; `fill_missing_tool_results` |
| `client.rs` — model streaming | `tapestry-protocol::model` (types), `tapestry-core::model` (behavior) | `ModelRequest`/`ModelResponse`; `ModelClient` trait + `MockModelClient` |
| prompt rendering | `tapestry-core::prompt` | `render_user`, `render_environment_context`, `render_system_reminder` |
| config / session settings | `tapestry-core::config` + `ThreadSettings` | Sparse overrides |
| tool execution | `tapestry-core::tool`, `tapestry-protocol::tool` | `Tool` trait, `ToolRegistry`; `ToolSpec`, `ToolCall`, `ToolResult` (protocol) |

### Protocol layers

`tapestry-protocol` owns every protocol type and groups them by layer; `tapestry-core` keeps only behavior (traits, state machine, IO, policy, rendering).

```text
ids <- common <- tool <- history <- { model, react }
client (submission, event) -> { ids, common, tool }   // never react
```

| Layer | Contents |
|-------|----------|
| `ids` | `SubmissionId`, `TurnId`, `ThreadId` (UUID); `ToolCallId` (opaque string) |
| `common` | `TokenUsage`, `FinishReason` |
| `tool` | `ToolSpec`, `ToolCall`, `ToolCallStatus`, `ToolResult` |
| `history` | `HistoryItem`, `EnvironmentContext`, `SystemReminder` |
| `model` | `ModelRequest`, `ModelResponse` (backend-neutral model protocol) |
| `react` | `TurnStep`, `ModelOutputEffect` (driver ↔ state machine) |
| `client` | `Submission`, `Op`, `Event`, `EventMsg`, `ThreadSettings` |

- `ToolCallId` is the provider's original call id; it is generated (`call_` + UUID) only when the provider gives none. Treat it as opaque.
- `ToolResult { call_id, tool_name, output, status }` is the single result type: tools return it, history stores it, `ToolCallEnd` carries it. After a tool runs, `AgentLoop` overwrites `call_id`/`tool_name` with those of the call.
- `TokenUsage { input_tokens, output_tokens, cache_read_tokens, cache_write_tokens }` (`u64`). `input_tokens` excludes cache reads/writes (genai's total prompt tokens are split accordingly); `output_tokens` includes reasoning. `total_tokens` is derived: written on serialization, ignored on deserialization.

## Turn execution

### State machine and driver

`TurnMachine` is synchronous and performs no IO. It exposes `next_step()` (`CallModel`, `RunTools`, `Done`) and is advanced by `on_model_output` / `on_tool_results`, which also write the turn's items to history. `AgentLoop::run_turn` is the async driver: it performs the step, times tools, and emits events. `RunTools` is a batch so parallel execution can be added later.

- A model output without tool calls ends the turn with `finish_reason: completed`.
- Tool calls are executed in order and their results fed back; unknown tools (`not_found`) and failures (`failed`) are results, not errors, and the loop continues.
- After `max_tool_iterations` model calls that still requested tools, the machine runs those tools, then makes one more call with no tools and a transient `SystemReminder::ToolLimitReached` appended to that request's history copy only (never stored). The turn ends with `finish_reason: exceed_max_iters`; tool calls in that final output are ignored and a `Warning` is emitted.
- On model failure or a closed event channel the turn returns an error (the thread emits `Error`), and every unanswered tool call in history receives a `failed` result so later requests stay valid.

### History and names

The thread session owns a `Vec<HistoryItem>` (`User{name}`, `Assistant{content, tool_calls}`, `ToolResult`, `EnvironmentContext`) that persists across turns; `SystemReminder` only appears in request copies. `ModelRequest` carries `system_prompt`, `history`, `tools` (empty when tools are disallowed) and `iteration`.

- `Op::StartTurn.name` becomes `User.name`; it must be non-empty and contain no line breaks, otherwise the thread emits `Error` and history is unchanged.
- Adapters render user messages with `prompt::render_user` as `"{name}: {content}"` (a text prefix, not a provider `name` field); consecutive user messages are not merged.
- Assistant items never carry a name. The agent's own name (`ThreadSettings.agent_name`) is appended to the system prompt as `Your name is {name}.`
- Known limit: user content can imitate the `name:` prefix.

### Time context

Models have no clock, so `run_turn` appends a `HistoryItem::EnvironmentContext` before the turn's user message when the date or time zone differs from the baseline (design follows Codex `<environment_context>`):

- `EnvironmentContext` is `Available{date, timezone}` or `Unavailable`; every entry is a complete snapshot, and `prompt::render_environment_context` renders it as `<environment_context>` text for the model. The baseline is the last such entry in history (none means nothing sent), so no session state is kept. Same-day turns send nothing; detection happens once per turn, not per model call.
- Time comes from an injected `Arc<dyn Clock>` (`SystemClock` by default: local date plus IANA zone, falling back to a UTC offset; `FixedClock` for tests).
- `ThreadSettings.include_time_context` (default on) disables it. Disabling after a send, or a clock failure, appends `<current_date status="unavailable" />` once; a clock failure never fails the turn. Recovery re-sends the full context.
- It differs from `SystemReminder`: this entry is persisted in history, the reminder only exists in a single request clone.
- No timestamp field or event is added; event order is unchanged.
- History rewrites (compaction, rollback, restore) need no reset: the baseline follows whatever history remains, so a rewrite that drops the entries re-sends in full and one that keeps them does not.
- Future persistence: timestamps belong in the storage envelope, not on items. Never derive the baseline from envelope timestamps; skip unknown entry types.

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

All events carry the triggering `SubmissionId`. `ToolCallEnd` carries the `ToolResult` (with `status`: `completed`, `failed`, `not_found`) and `duration_ms`; `ModelRequestCompleted` and `TokenCount` carry `TokenUsage` (the latter summed over the turn).

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
            Loop->>Machine: on_tool_results
        end
    end
    Loop-->>Client: TokenCount
    Loop-->>Client: TurnComplete
```

### Current limits and next phases

- The genai mapping drops provider metadata such as Gemini `thought_signatures`, which may break multi-call tool use with thinking models; the model protocol needs a provider-metadata field.
- Turns are serial: `Interrupt` is queued and only handled after the running turn ends; in-turn cancellation is a later phase.
- History is in memory and unbounded (no compaction).
- Not yet: streaming output, parallel tools, hooks, approvals, multi-agent messaging.

## v1 protocol surface

### Operations (`Op`)

- `Configure` — initialize thread model + system prompt
- `StartTurn` — run one user turn (`content`, optional `name`, optional `turn_id`)
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
