# Tapestry Architecture

Tapestry is a composable Rust framework for building cross-platform AI agents. This document maps the first-version core layout to [OpenAI Codex](https://github.com/openai/codex) (`codex-rs/`), which informed the design.

> **Attribution:** SQ/EQ communication, turn lifecycle, and event taxonomy follow patterns from Codex `codex-protocol` and `codex-core`. Tapestry reimplements them idiomatically for this framework; it is not a fork.

## Workspace layout

| Crate | Codex analogue | Responsibility |
|-------|----------------|----------------|
| `tapestry-protocol` | `codex-protocol` | Submission Queue / Event Queue types (`Submission`, `Command`, `Event`, `EventMsg`), IDs, thread settings |
| `tapestry-core` | `codex-core` | Thread lifecycle, agent loop, model client trait, tool dispatch |
| `tapestry-adapter-agui` | — | Maps Tapestry events to the AG-UI 1.0 event schema |
| `tapestry-adapter-a2a` | — | Maps Tapestry events to the A2A 1.0 streaming response schema |
| `tapestry-adapter-cli` | — | Filters and renders Tapestry events as human-readable command-line text (tool calls, tool results and assistant messages only) |
| `tapestry-cli` | `codex-cli` | Loads model settings and an optional agent from YAML (default system prompt `You are a helpful assistant.`), submits one turn, and prints tool calls, tool results and assistant messages as text lines (via `tapestry-adapter-cli`) |

Future crates (not in scope for ALGO-14):

| Planned crate | Codex analogue | Notes |
|---------------|----------------|-------|
| `tapestry-app-server` | `codex-app-server` | JSON-RPC long process for IDE/SDK integration |
| `tapestry-sandbox-*` | sandbox crates | Platform sandboxing and process hardening |

## Communication model

Clients send commands through a bounded submission channel and read results from the thread's append-only **event log**:

```text
 Client                              TapestryThread
   |                                        |
   |  Submission (SQ)                       |
   |--------------------------------------->|
   |                                        | process Command
   |  Event (EQ)                            |
   |<---------------------------------------|
```

- **Submission Queue (SQ):** client → agent requests (`Command`)
- **Event log (EQ):** agent → client updates (`EventMsg`), persisted in an `EventStore` and read through subscriptions

Each `Submission` carries a correlating `SubmissionId`; every `Event` echoes that id so clients can match responses to requests.

## Event sourcing

The event log is the single source of truth for a thread. Conversation history is not stored separately; it is a projection folded from the log.

```text
 AgentLoop / thread worker
        | record(msg)
        v
    Recorder --append--> EventStore (per-thread log, positions 1, 2, 3, ...)
        |                     |
        | apply (on success)  +--subscribe(after_log_position)--> EventSubscription (any number)
        v
 formatter::formatter_trait::ConversationProjection --messages()--> model-facing formatter view
 EventStore --read_after(thread, 0)--> ModelRequest.events (ordered immutable snapshot)
```

### Envelope

`Event { id, log_position, thread_id, turn_id, timestamp_ms, msg }`:

- `id` — the `SubmissionId` that caused the event.
- `log_position` — assigned by the store; starts at 1 per thread with no gaps.
- `turn_id` — set on every event inside a turn (and on an `Error` raised in one); omitted otherwise. Payloads never repeat it.
- `timestamp_ms` — Unix milliseconds from the thread's `Clock::unix_millis()`.

### Recorder

`tapestry-core::recorder::Recorder` is the only writer. `record(msg)` appends a `NewEvent` (stamped with the current submission, turn and time) and, only after the append succeeds, applies the stored event to its live `formatter::formatter_trait::ConversationProjection`. The thread worker calls `begin_submission` for each submission (clearing the turn), and `run_turn` calls `set_turn` before `TurnStarted`.

### EventStore and subscriptions

`EventStore` (`tapestry-core::store`, `async_trait`, `Send + Sync`) has `append`, `read_after(thread, after_log_position)` and `subscribe(thread, after_log_position)`. Backends are pluggable; `InMemoryEventStore` is the default (cheap to clone, shared log per thread, `tokio::sync::watch` wake-ups).

- `EventSubscription::next()` replays stored events after the cursor, then waits for new ones; it never drops events however slow the consumer is, and any number of subscribers can read the same thread. It ends (`None`) when the store is gone.
- `EventSubscription::collect_events_for(id)` reads until a terminal event for that submission (`TurnComplete`, `TurnAborted`, `Error`, `SessionConfigured`). The result may include other submissions' events, so correlate by `Event.id`.
- A store failure ends the current submission. The thread attempts to record an `Error` with the original submission id; if the store rejects that append too, the thread logs the failure and continues.

### History projection

`ConversationProjection::apply` maps events to `ConversationItem`s: `UserMessage` → `User`, `AssistantMessage` → `Assistant`, `ToolCallEnd` → `ToolResult`, `EnvironmentContextUpdated` → `EnvironmentContext`. `SystemReminder` events are held apart as pending reminders: the first following `ModelRequestCompleted` consumes them and a `TurnStarted` discards any unconsumed ones, so the position of a reminder in the log never decides whether it is delivered. `messages()` returns the items followed by the pending reminders in record order, so reminders always come last and never sit between a tool call and its result. `AssistantMessageStart` and `AssistantMessageDelta` are ignored: only the final `AssistantMessage` produces an entry, so the partial text of a message whose turn failed mid-stream never reaches the conversation.

Repair happens only in the projection (no compensating events): when a turn is closed by `TurnAborted`, by an `Error` carrying a `turn_id`, by the next `TurnStarted`, or by the end of `fold`, every unanswered tool call of that turn receives a synthetic `failed` result. Synthetic results are inserted before still-valid trailing reminders. `fold` over the same log always produces the same history.

### Resume

`TapestryThread::builder(model, tools).event_store(store).resume(thread_id)` reads the log, folds the projection and continues the same thread; new events continue the log positions. Thread configuration is not persisted, so a resumed thread must receive `UpdateSettings` again before `StartTurn`.

## Module mapping (Codex → Tapestry)

| Codex (`codex-core` / `codex-protocol`) | Tapestry | Notes |
|-------------------------------------------|----------|-------|
| `protocol.rs` — `Submission`, `Op`, `Event`, `EventMsg` | `tapestry-protocol::{submission, event}` | `Command` replaces `Op`; reduced command/event set for v1 |
| `codex_thread.rs` — thread lifecycle | `tapestry-core::thread` | `TapestryThread`, `ThreadBuilder`, `ThreadHandle` |
| rollout recording | `tapestry-core::{store, recorder}` | `EventStore`, `InMemoryEventStore`, `EventSubscription`, `Recorder` |
| `agent/` — turn policy & driver | `tapestry-core::agent` | `Agent` trait (`reasoning`/`acting`/`forward` + `*_with_hooks`; `ReactAgent` by default) + `AgentLoop::forward` (turn prologue) |
| conversation history | `tapestry-protocol::model` (`ConversationItem`), `tapestry-core::formatter::formatter_trait` | `ConversationProjection` folds events and repairs unfinished turns |
| model invocation | `tapestry-protocol::model` (types), `tapestry-core::model` (behavior) | `ChatModel` (`complete`, `stream`), `ResponseModel` (`respond`, `stream`), shared `ModelStream`, and a mock for each trait |
| provider integrations | `tapestry-core::model::genai` | Provider configuration, request/response mapping, and the genai-backed `GenaiModelClient` |
| prompt rendering | `tapestry-core::formatter::utils` | `render_user`, `render_environment_context`, `render_system_reminder` |
| config / session settings | `tapestry-core::config` + `ThreadSettings` | Sparse overrides |
| tool execution | `tapestry-core::tool`, `tapestry-core::builtin_tools`, `tapestry-protocol::tool` | `Tool` trait, `ToolRegistry`; `ToolSpec`, `ToolCall`, `ToolResult` (protocol) |

`formatter::Formatter` provides default event-stream folding (`events_to_items`) into `ConversationItem`s via `ConversationProjection`, then default conversion into `genai::chat::ChatMessage`s; `format` invokes the two stages in order. Implementors can override either stage. The default message view maps every projected item in order, with no position-based reminder filtering. Unfinished tool calls are repaired when a turn ends; the synthetic results are appended to the items, ahead of the pending reminders. `OpenAiFormatter` uses these defaults. Shared text rendering helpers live in `formatter::utils`; `formatter::prompt` remains a compatibility re-export. `ModelRequest.events` is a complete ordered snapshot read from the event store after `ModelRequestStarted` and every reminder queued for this request have been appended. `GenaiModelClient` owns an `Arc<dyn Formatter>` and uses it for both streaming and non-streaming requests; OpenAI Chat and Responses models expose `with_formatter` and default to `OpenAiFormatter`. The public `model_request_to_chat_request` helper uses that default strategy. `model::chat_model` and `model::response_model` each group a trait with its mock and a concrete OpenAI client. `OpenAiChatModel` pins Chat Completions; `OpenAiResponseModel` pins the Responses API and sends the local event snapshot on each request. The traits share the normalized protocol types and `ModelStream`. `AgentLoop` uses `ChatModel`; the response interface is not wired into the agent loop. `ModelClient` and `MockModelClient` remain compatibility aliases for `ChatModel` and `MockChatModel`.

### Protocol layers

`tapestry-protocol` owns Tapestry's protocol types and groups them by layer; external wire types and mappings live in their own adapter crates. `tapestry-core` keeps runtime behavior (traits, state machine, IO, policy, rendering).

```text
common (incl. ids, context) <- tool <- model (incl. ConversationItem)
submission, event -> { common, tool }
adapter -> event
```

| Layer | Contents |
|-------|----------|
| `common` | `TokenUsage`, `FinishReason`; `common::ids`: `SubmissionId`, `TurnId`, `ThreadId` (UUID), `ToolCallId` (opaque string); `common::context`: `EnvironmentContext`, `SystemReminder` |
| `tool` | `ToolSpec`, `ToolCall`, `ToolCallStatus`, `ToolResult` |
| `model` | `ModelRequest`, `ModelResponse`, `ConversationItem` (backend-neutral model protocol) |
| `submission` | `Submission`, `Command`, `ThreadSettings` |
| `event` | `Event`, `EventMsg` and the event payloads |
| `adapter` | Protocol-independent event categories, filters and the `EventAdapter` conversion contract; depends on `event`, not on core or a target protocol |

`TurnStep` is an internal driver-policy type in `tapestry-core::agent`, not a protocol type.

- `ToolCallId` is the provider's original call id; it is generated (`call_` + UUID) only when the provider gives none. Treat it as opaque.
- Event adapters live in separate protocol-specific crates. They consume one thread's events in sequence and may map each input event to zero or more outputs; `EventAdapter::finish` flushes terminal output when a subscription ends. `EventFilter` selects intrinsic event categories consistently across adapters. `tapestry-adapter-cli`'s `CliEventAdapter` emits a `CliOutput` only for `ToolCallBegin`, `ToolCallEnd` and `AssistantMessage` with text, and drops everything else; its `Display` renders `tool call: name(args)`, `tool result: name: output` (with `(failed)`/`(not found)` after the name when not completed) and the assistant content as-is. `CliAdapterOptions::include_deltas` additionally emits raw `AssistantMessageDelta` text. `tapestry-cli` uses the default options and still detects submission completion from the unfiltered events, reporting `Error`/`TurnAborted` details on stderr with a failing exit status.
- `ToolResult { call_id, tool_name, output, status }` is the single result type: tools return it, `ToolCallEnd` carries it, and the projection stores it in history. After a tool runs, `AgentLoop` overwrites `call_id`/`tool_name` with those of the call.
- `TokenUsage { input_tokens, output_tokens, cache_read_tokens, cache_write_tokens }` (`u64`). `input_tokens` excludes cache reads/writes (genai's total prompt tokens are split accordingly); `output_tokens` includes reasoning. `total_tokens` is derived: written on serialization, ignored on deserialization.

## Turn execution

### State machine and driver

`Agent` is an async, per-turn execution interface (`Send + Sync`). Implementors provide `AgentDeps` (model, `ToolRegistry`, middleware); the default methods implement the standard ReAct driver and can be overridden at three levels:

- `reasoning(ctx, request: ModelRequest)` is the single model execution point. Its default implementation consumes `ModelClient::stream`, records text deltas, and returns a `ReasoningOutput`. It does not expose a `Recorder` to the model implementation.
- `acting(call: &ToolCall)` is the single-tool execution point. Its default implementation calls the matching registered tool and returns `NotFound` for unknown tools.
- `forward(ctx) -> TurnOutcome` is the unified default scheduler for the closed `TurnStep::{CallModel, CallTool, Done}` set. `CallTool` consumes the pending tool calls stored in `TurnContext`; it cannot be repeated or fabricated. A custom `forward` can define a different flow while keeping the standard `forward_with_hooks` wrapper.

The standard path is `AgentLoop::forward` -> `Agent::forward_with_hooks` -> `Agent::forward`. `forward_with_hooks` applies `before_turn`, calls the overridable `self.forward`, then applies `after_turn` and records exactly one `TokenCount`/`TurnComplete`. The default `forward` itself uses the hooked model and tool steps, so calling `forward` does not disable middleware. It returns an outcome and does not record turn completion.

`TurnContext` is per-turn state: it carries the `Recorder`, `ThreadConfig`, `TurnId`, iteration, accumulated usage, initial plan, `pending_tool_calls`, whether a model iteration has started, and a reminder queue. `remind(reminder)` queues a `SystemReminder`; `flush_reminders()` records the queued reminders in order, one `SystemReminder` event each. A model iteration stays started while its pending tool calls execute; `ModelRequestCompleted(i)` is emitted only after the scheduling decision for those calls. Model calls are capped at `max_tool_iterations + 1`; the last default call has tools disabled and follows a `ToolLimitReached` reminder.

A custom `forward` is responsible for preserving the same observable event and budget contracts for work it performs. `TurnContext::begin_model_iteration` and `finish_model_iteration` provide the shared model-boundary operations; `finish_model_iteration` records `ModelRequestCompleted` and adds that call's usage to the turn total. Pending tool calls likewise have explicit set/take methods.

The model step records `ModelRequestStarted`, flushes queued reminders, reads the event snapshot, runs the read-only `before_model` hooks, flushes again and appends those reminders to the snapshot, calls `self.reasoning(ctx, request)`, then runs the read-only `after_model` hooks. The tool-call wrapper calls `before_tool`, records `ToolCallBegin`, invokes `self.acting(call)` (or uses a `Skip` result), runs `after_tool`, re-keys the result to the original call and records `ToolCallEnd` for each call in order. A failure stops the remaining calls; a failed/not-found/skipped result by itself does not.

After each successful model step and each execution of the pending tool calls, the scheduler computes a default next step and runs `after_step` with a read-only `StepOutput` and that mutable plan. Later middleware can replace a nonterminal plan. `Done` is locked when it is the default or first selected; later middleware still observes it but cannot change the termination or its outcome. Invalid transitions are runtime errors. If the selected model step does not execute its returned tool calls, the complete assistant message omits those calls, preserves text and records a warning with the ignored count. This avoids dangling calls in history.

`AgentLoop` starts turns: it records `TurnStarted`, the optional environment context and the user message, then creates a fresh agent with its `AgentFactory` (`Fn(AgentDeps, &ThreadConfig) -> Box<dyn Agent>`, receiving the loop's model, tools and middleware) and runs `forward_with_hooks`. Install custom agents with `AgentLoop::with_agent_factory` and inject the configured loop with `ThreadBuilder::agent_loop(AgentLoop)`.

### Middleware hooks

Middleware is bound through `AgentDeps` or `AgentLoop::with_middleware` /
`with_middlewares`
bind it to every agent the loop creates, and the constructors default to none.
All hooks run in middleware registration order, including `after_turn`,
`after_model`, `after_tool` and `after_step`. `HookCtx` provides the thread id,
turn id, optional model/tool iteration and read-only thread configuration.
Middleware cannot access the `Recorder`. The only way a hook affects the model
context is `HookCtx::remind(reminder)`, which queues a system reminder that core
records as a `SystemReminder` event at a safe point: never between an assistant
message's start and final events, nor between one tool's `ToolCallBegin` and
`ToolCallEnd`. Reminders from `before_turn`, `before_model` and the default
scheduler are recorded before the next request's snapshot, so that request
delivers them. Reminders written while a request is open (`after_model`, tool
hooks, `after_step`) are recorded right after its `ModelRequestCompleted`
(otherwise that event would consume them) and reach the next request; if the
turn ends first they are recorded before `TokenCount` and expire at the next
`TurnStarted`. `after_turn` reminders are also recorded before `TokenCount`.

- `before_turn` runs after `TurnStarted`, optional environment context and
  `UserMessage`, and can change the initial step. Selecting `Done` skips model
  and tool work; `after_turn` still runs before completion events.
- `before_model(&HookCtx, &[Event])` runs after the request snapshot is
  assembled and before `self.reasoning`; `after_model(&HookCtx, &ModelResponse)`
  runs after the final response, before the default plan is computed. Both are
  read-only: they cannot rewrite the request, system prompt, tool specs or
  response, and cannot add tool calls. Content review belongs in the model
  implementation or a `reasoning` override.
- `before_tool` runs before `ToolCallBegin`. It may rewrite arguments or return
  `ToolDecision::Skip(result)` to bypass execution and the remaining
  `before_tool` hooks for that call. The original call id and name are restored
  before recording and invoking `self.acting`. `after_tool` runs before
  `ToolCallEnd`, including for skipped and unknown tools; result id and name are
  restored from the original call afterward.
- `after_step` runs once after a successful model response or complete tool
  batch, with the post-data-hook result and default next step. It can normally
  end after the batch, but cannot interrupt a batch or reopen `Done`.
- Any hook error stops the turn and propagates to the thread worker, which
  records the submission-scoped `Error`. `after_turn` is not called after a
  prior hook or runtime error.

Migration from rewriting model hooks: use `HookCtx::remind` to influence the
next request, `after_step` to change scheduling, and a `reasoning` override or
the model implementation for output review.

Migration from the former synchronous `next_step`/`on_model_output`/
`on_tool_results` policy is to override `forward` for a genuinely custom flow,
or use `after_step` to adjust the standard scheduler. Override `reasoning` for
model behavior and `acting` for one tool call; their `*_with_hooks` wrappers
continue to run. The old reverse order for post-hooks is replaced by
registration order.

Middleware-owned state is retained by the middleware instance (for example
through atomics or a mutex), is not stored in the event log, and is not restored
when resuming a thread.

### Streaming model output

`AgentLoop` always calls `ChatModel::stream`, which returns a `ModelStream` of `ModelStreamItem`s: zero or more `TextDelta { text }`, then exactly one `Completed { response }` carrying the full text, tool calls and usage. The default `stream` wraps `complete`, so non-streaming clients need no changes. Both mocks stream text word by word; `model::genai::GenaiModelClient` uses genai's `exec_chat_stream` with content, tool-call and usage capture (text chunks become deltas, the end event becomes `Completed`; a stream ending without it is a model error).

- Each assistant message gets a core-generated `MessageId`, shared by its `AssistantMessageStart`, `AssistantMessageDelta`s and final `AssistantMessage`; every model output uses a new id.
- `AssistantMessageStart` is recorded at the first non-empty delta, or just before the final message when there were no deltas. An output with no text, no tool calls and no deltas records none of these events.
- The final text comes from the `Completed` response; only when it has none are the deltas concatenated. Consumers must not validate the final text against the deltas.
- If the stream fails after deltas, no `AssistantMessage` is recorded and the turn ends with `Error`.
- Tool-call argument deltas and reasoning deltas are not streamed.

- A model output without tool calls ends the turn with `finish_reason: completed`.
- Tool calls are executed in order and their results fed back; unknown tools (`not_found`) and failures (`failed`) are results, not errors, and the loop continues.
- The default `ReactAgent` runs tools for up to `max_tool_iterations` model calls, then makes one more call with no tools after queuing a `SystemReminder::ToolLimitReached`, recorded before that request's snapshot and formatted at the end of its messages. The completed request consumes the reminder; it stays only in the log. The turn ends with `finish_reason: exceed_max_iters`; tool calls in that final output are ignored and a `Warning` is emitted. Custom policies may choose different turn flows but cannot exceed `max_tool_iterations + 1` model calls.
- On model failure the turn returns an error (the thread records `Error` with the turn's `turn_id`), and the projection gives every unanswered tool call a `failed` result so later requests stay valid. On an event-store failure, the thread attempts to record the submission error; an unavailable store may reject that error append too, and the open turn is repaired when the next turn starts or the log is folded.

### History and names

History (`User{name}`, `Assistant{content, tool_calls}`, `ToolResult`, `EnvironmentContext`, `SystemReminder`) is the `ConversationProjection` of the event log and persists across turns; unconsumed reminders are appended last. `ModelRequest` carries `system_prompt`, the complete ordered `events` snapshot, `tools` (empty when tools are disallowed) and `iteration`.

- `Command::StartTurn.name` becomes `User.name`; it must be non-empty and contain no line breaks, otherwise the thread emits `Error` and history is unchanged.
- Adapters render user messages with `prompt::render_user` as `"{name}: {content}"` (a text prefix, not a provider `name` field); consecutive user messages are not merged.
- Assistant items never carry a name. The agent's own name (`ThreadSettings.agent_name`) is appended to the system prompt as `Your name is {name}.`
- Known limit: user content can imitate the `name:` prefix.

### Time context

Models have no clock, so `run_turn` records an `EnvironmentContextUpdated` event (projected to `ConversationItem::EnvironmentContext`) before the turn's user message when the date or time zone differs from the baseline (design follows Codex `<environment_context>`):

- `EnvironmentContext` is `Available{date, timezone}` or `Unavailable`; every entry is a complete snapshot, and `prompt::render_environment_context` renders it as `<environment_context>` text for the model. The baseline is the last such entry in history (none means nothing sent), so no session state is kept. Same-day turns send nothing; detection happens once per turn, not per model call.
- Time comes from an injected `Arc<dyn Clock>` (`SystemClock` by default: local date plus IANA zone, falling back to a UTC offset; `FixedClock` for tests).
- `ThreadSettings.include_time_context` (default on) disables it. Disabling after a send, or a clock failure, appends `<current_date status="unavailable" />` once; a clock failure never fails the turn. Recovery re-sends the full context.
- It differs from `SystemReminder`: both are projected from events, but time context is a baseline snapshot while reminders are consumed by the next completed model request.
- History rewrites (compaction, rollback, restore) need no reset: the baseline follows whatever history remains, so a rewrite that drops the entries re-sends in full and one that keeps them does not.
- Timestamps live on the event envelope (`timestamp_ms`), not on items. Never derive the baseline from envelope timestamps.

### Event order

```text
TurnStarted
[EnvironmentContextUpdated]
UserMessage
  ModelRequestStarted(i)
    [SystemReminder]*                -- queued reminders; recorded before the request snapshot, or after the previous ModelRequestCompleted
    [AssistantMessageStart
      AssistantMessageDelta*         -- streamed text, same message_id
      AssistantMessage]              -- omitted entirely for an empty output
    [Warning]
    [ToolCallBegin, ToolCallEnd]*   -- paired per call, in call order
  ModelRequestCompleted(i)
  ... repeated for each model call ...
TokenCount
TurnComplete { finish_reason, last_agent_message }
```

All events carry the triggering `SubmissionId`, and turn events carry the `turn_id` on the envelope. `ToolCallEnd` carries the `ToolResult` (with `status`: `completed`, `failed`, `not_found`) and `duration_ms`; `ModelRequestCompleted` and `TokenCount` carry `TokenUsage` (the latter summed over the turn).

```mermaid
sequenceDiagram
    participant Client
    participant Thread as TapestryThread
    participant Loop as AgentLoop
    participant Agent
    participant Model as ModelClient
    participant Tools as ToolRegistry
    participant MW as Middleware

    Client->>Thread: Command::UpdateSettings (first)
    Thread-->>Client: SessionConfigured

    Client->>Thread: Command::StartTurn
    Thread-->>Client: TurnStarted, UserMessage
    Thread->>Agent: create per-turn Agent and TurnContext
    Agent->>MW: before_turn(initial_step)
    loop until Done
        alt CallModel
            Agent->>MW: before_model(events)
            Agent->>Model: reasoning(ModelRequest)
            Agent->>MW: after_model(response)
            Agent->>MW: after_step(Model response, next_step)
            Agent-->>Client: AssistantMessageStart / AssistantMessageDelta* / AssistantMessage
        else CallTool
            loop each call in complete ordered batch
                Agent->>MW: before_tool
                Loop-->>Client: ToolCallBegin
                Agent->>Tools: acting(single call)
                Agent->>MW: after_tool
                Loop-->>Client: ToolCallEnd
            end
            Agent->>MW: after_step(complete tool results, next_step)
        end
    end
    Note over Agent,Loop: ModelRequestCompleted(i) closes after the model's tool calls and its step hook
    Agent->>MW: after_turn
    Loop-->>Client: TokenCount
    Loop-->>Client: TurnComplete
```

### Current limits and next phases

- The genai mapping drops provider metadata such as Gemini `thought_signatures`, which may break multi-call tool use with thinking models; the model protocol needs a provider-metadata field.
- Turns are serial: `Interrupt` is queued and only handled after the running turn ends; in-turn cancellation is a later phase.
- Only an in-memory event store ships; history is unbounded (no compaction).
- Not yet: tool-argument and reasoning deltas, parallel tools, approvals, multi-agent messaging.

## v1 protocol surface

### Commands (`Command`)

- `UpdateSettings` — apply sparse overrides (`model`, `system_prompt`, `max_tool_iterations`, `agent_name`, `include_time_context`); the only way to configure a thread. Only the first one emits `SessionConfigured`; later ones emit no confirmation event, so use `EventSubscription::collect_events_for` only for the first.
- `StartTurn` — run one user turn (`content`, optional `name`, optional `turn_id`)
- `Interrupt` — abort the in-flight turn (`TurnAborted` with reason `interrupted`)

### Events (`EventMsg`)

- `SessionConfigured`, `TurnStarted`, `UserMessage`, `AssistantMessageStart`, `AssistantMessageDelta`, `AssistantMessage` (one complete assistant message: `message_id`, optional text and tool calls)
- `EnvironmentContextUpdated`, `SystemReminder`
- `ModelRequestStarted`, `ModelRequestCompleted`, `Warning`
- `ToolCallBegin`, `ToolCallEnd`
- `TokenCount`, `TurnComplete`, `TurnAborted`, `Error`

## Cross-platform rules

- `tapestry-protocol` and `tapestry-core` must not depend on crates that build on only one OS or download platform binaries at build time; isolate such code behind a feature or separate crate.
- `tapestry-protocol` must build for `wasm32-unknown-unknown`; `uuid` uses its `js` feature on that target for randomness (`getrandom` version follows `uuid`).
- CI checks the workspace on Linux, macOS and Windows, checks `tapestry-protocol` on wasm32, and keeps fmt, clippy and tests on Linux.

## Testing strategy

- **Protocol:** serde round-trip unit tests for every public request/event type
- **Core:** `TurnContext` tests cover per-turn state and single-use pending tool calls; `AgentLoop` tests cover overridden reasoning/tool/forward methods, middleware registration order, `after_step` scheduling and early finish, tool-budget and event invariants, and scripted model streams; `InMemoryEventStore` and `ConversationProjection` have their own unit tests (ordering, replay, repair); `MockModelClient` drives deterministic paths (echo + tool loop, streamed word by word)
- **Integration:** `TapestryThread` end-to-end tests via `ThreadHandle` and `EventSubscription`, including resume from a stored log

Run locally:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

### Built-in local tools

`tapestry-core::builtin_tools` implements `ReadFileTool`, `WriteFileTool`, and
`ShellTool` through the existing `Tool` contract. `BuiltinToolsConfig` selects
individual tools; its default enables none and `all(directory)` enables all
three. The CLI registers all three against its working directory by default.
An agent template can supply `allow_tools` to register only the named tools;
an empty list registers none. Unknown names and invalid list values are rejected
at configuration load time. Only registered tools are advertised and executable.
Library registries remain empty unless the caller registers tools. No protocol,
event ordering, or agent scheduling changes are required. Local tools use host
permissions rather than a sandbox. File tools validate regular-file metadata
before opening and on the opened handle, and truncate only after validation.
Unix opens use `O_NONBLOCK` to prevent FIFO replacement races from blocking.
Shell captures bounded stdout/stderr and
applies a timeout, with direct-child kill-on-drop. Descendant process isolation
is not provided.
