# Design

## Context

动机见 proposal.md 的 Why 部分，行为契约见 specs/。当前与本设计相关的状态如下：

- `tapestry-protocol` 的模块为 `context`、`error`、`event`、`history`、`ids`、`submission`、`tool`，根部用 `pub use *` 平铺重导出。`ToolCallStatus` 和 `FinishReason` 定义在 `event.rs`，但 history 和 core 都在使用。
- core 中协议性质的类型有：`model.rs`（`ModelRequest/Response`）、`turn.rs`（`TurnStep`、`ToolOutcome`、`ModelOutputEffect`）、`tool.rs`（`ToolResult{success}`、`ToolContext`）、`agent.rs`（`UserInput`）。
- 环境上下文和强制收尾提示目前都以 `HistoryItem::User{name: None}` 的字符串形式写入（agent.rs:99、150）。`ContextState` 单独保存在 `Session` 中，`replace_history` 时重置。
- `ToolCallId` 由 `id_type!` 生成，是 UUID newtype。genai 映射会丢弃提供方的 `call_id`（mapping.rs:82）。
- 项目没有任何持久化数据，线格式也没有外部消费者，因此可以一次性做不兼容修改。

## Goals / Non-Goals

**Goals:**
- protocol 内部按层组织模块，模块依赖方向清晰，可以在编译期验证。
- 每个概念只保留一个类型：工具结果、用户消息、token 用量。
- 结构化条目的渲染集中在 core 的一个模块中，所有 `ModelClient` 实现共用。

**Non-Goals:**
- 不处理 Gemini 的 `thought_signatures` 等提供方元数据（已作为后续事项记录在 proposal）。
- 不引入流式输出、回合内中断或持久化存储。
- 不保留旧线格式的兼容层。

## Decisions

### D1. protocol 模块布局与依赖方向

```
tapestry-protocol/src/
├── lib.rs        根部重导出所有公开类型
├── ids.rs        SubmissionId、TurnId（id_type!）；ToolCallId（字符串 newtype）
├── error.rs
├── common.rs     TokenUsage、FinishReason
├── tool.rs       ToolSpec、ToolCall、ToolCallStatus、ToolResult
├── history.rs    HistoryItem、EnvironmentContext、SystemReminder
├── model.rs      ModelRequest、ModelResponse
├── react.rs      TurnStep、ModelOutputEffect
└── client/
    ├── mod.rs
    ├── submission.rs   Submission、Op
    └── event.rs        Event、EventMsg、*Event、ThreadSettings、TurnAbortReason

依赖：ids ← common ← tool ← history ← { model, react }
      client → { ids, common, tool }   （禁止依赖 react）
```

各层采用单文件还是目录形式：只有 `client` 内容较多，因此拆分为目录，其余层保持单文件。

`context.rs`（`ContextState`）删除。

备选方案：沿用平铺的文件，只靠命名区分各层。没有采用的原因是，模块边界本身就是这次变更要表达的契约。

### D2. ToolCallId 改为字符串 newtype

`ToolCallId(String)` 使用 `#[serde(transparent)]`，提供 `From<String>`、`Display`、`as_str()`，以及 `ToolCallId::generate()`。`generate()` 生成 `call_` 加 UUID simple 格式的字符串，仅用作兜底，也供 Mock 使用。`ToolCallId` 不再走 `id_type!` 宏，宏只用于 UUID 类型的 ID。

`ToolCall::new(id, name, arguments)` 的 ID 由调用方传入：genai 映射传入提供方 ID，Mock 和测试使用 `ToolCallId::generate()`。

备选方案：保留 UUID，另外增加 `provider_call_id` 字段。没有采用，因为同一调用会有两个 ID，而目前没有需要区分二者的场景。

### D3. 统一的 ToolResult 及其构造方式

```rust
pub struct ToolResult { pub call_id: ToolCallId, pub tool_name: String, pub output: String, pub status: ToolCallStatus }
impl ToolResult {
    pub fn completed(call: &ToolCall, output: impl Into<String>) -> Self;
    pub fn failed(call: &ToolCall, output: impl Into<String>) -> Self;
    pub fn not_found(call: &ToolCall) -> Self;
}
```

- `HistoryItem::ToolResult(ToolResult)` 采用 newtype 变体。由于使用内部标签，JSON 依然是平铺的 `{"type":"tool_result","call_id":…}`。
- `ToolCallEndEvent { turn_id, result: ToolResult, duration_ms }`。
- `Tool::execute(&self, call: &ToolCall) -> ToolResult`，删除 `ToolContext`（`turn_user_message` 目前没有任何读取方）。
- AgentLoop 在 `execute` 返回后，用原调用的值覆盖 `call_id` 和 `tool_name`。`TurnMachine::on_tool_results(Vec<ToolResult>)` 保留顺序和 ID 校验，作为防御内部缺陷的手段。
- 删除 `ToolOutcome`。

### D4. history 中的结构化条目

```rust
enum HistoryItem {                       // tag = "type"
    User { name: Option<String>, content: String },
    Assistant { content: Option<String>, tool_calls: Vec<ToolCall> },
    ToolResult(ToolResult),
    EnvironmentContext(EnvironmentContext),
    SystemReminder { reminder: SystemReminder },
}
enum EnvironmentContext {                // tag = "state"
    Available { date: String, timezone: String },
    Unavailable,
}
enum SystemReminder { ToolLimitReached } // snake_case 字符串
```

- `EnvironmentContext` 是 newtype 变体，内层使用另一个标签名 `state`，序列化结果为 `{"type":"environment_context","state":"available","date":…,"timezone":…}`。
- `SystemReminder` 只有单元变体，序列化结果是字符串而不是 map，无法作为内部标签的 newtype 变体。因此用结构体变体 `{ reminder }` 包装，序列化为 `{"type":"system_reminder","reminder":"tool_limit_reached"}`。
- 以上嵌套标签都需要往返测试来固定。

### D5. 删除 UserInput

`Op::StartTurn { content: String, name: Option<String>, turn_id: Option<TurnId> }`。线程校验 `name` 之后，把 `content` 和 `name` 交给 AgentLoop，由它构造 `HistoryItem::User`。如果以后用户消息需要支持多模态，再考虑抽出共享结构，本次不提前设计。

### D6. TokenUsage

```rust
#[serde(into = "TokenUsageWire", from = "TokenUsageWire")]
pub struct TokenUsage { pub input_tokens: u64, pub output_tokens: u64, pub cache_read_tokens: u64, pub cache_write_tokens: u64 }
impl TokenUsage { pub fn total_tokens(&self) -> u64; }
impl AddAssign for TokenUsage { /* 逐项 saturating_add */ }
```

- 私有的 `TokenUsageWire` 额外带一个 `total_tokens` 字段。序列化时由 `total_tokens()` 填充；反序列化时该字段为 `#[serde(default)]`，并在 `From` 转换中丢弃。四个计数都带 `#[serde(default)]`。
- `ModelResponse.usage`、`ModelRequestCompletedEvent.usage` 和 `TokenCountEvent.usage` 都使用该类型。AgentLoop 用 `+=` 在回合内累计。
- genai 映射：`cache_read = prompt_tokens_details.cached_tokens`，`cache_write = prompt_tokens_details.cache_creation_tokens`，`input = max(prompt_tokens − read − write, 0)`，`output = completion_tokens`（genai 已把 reasoning 计入其中）。所有 `Option`/负值都按 0 处理。

备选方案：`total_tokens` 作为普通存储字段。没有采用，因为可能与四项之和不一致。

### D7. ReAct 内部消息迁移

`TurnStep`（`#[serde(tag = "type", rename_all = "snake_case")]`）和 `ModelOutputEffect` 迁入 `react.rs` 并支持 serde。`TurnStep::CallModel.reminder` 的类型改为 `Option<SystemReminder>`。`TurnMachine`、`FORCED_FINISH_REMINDER` 的文案以及迭代上限判断都留在 core。

强制收尾时，AgentLoop 先克隆 history，再追加 `HistoryItem::SystemReminder`，然后发起请求。`ModelRequest` 中没有 reminder 字段。

### D8. core 的 prompt 渲染模块

新增 `tapestry-core/src/prompt.rs`，集中处理发给模型的文本：

- `render_user(name, content)`：原 `render_user_content`。
- `render_environment_context(&EnvironmentContext)`：可用时输出完整的 `<environment_context>` 片段，同时包含 `current_date` 与 `timezone`；不可用时输出 `status="unavailable"` 片段。
- `render_system_reminder(&SystemReminder)`：`ToolLimitReached` 映射为原来 `FORCED_FINISH_REMINDER` 的文案。

genai 映射中，`EnvironmentContext` 和 `SystemReminder` 都转换为 `ChatMessage::user(渲染文本)`。`MockModelClient` 查找最后一条 `User` 时，会自然跳过这两类条目，修复了此前 Mock 会把收尾提示当作用户消息回显的问题。

`context.rs` 只保留“是否需要追加条目”的判断，不再负责渲染。

### D9. 环境上下文基线从 history 推导

```rust
fn next_context_entry(history: &[HistoryItem], enabled: bool, clock: &dyn Clock) -> Option<EnvironmentContext>
```

函数从后往前查找最后一条 `EnvironmentContext` 作为基线：

| 基线 | 关闭 | 开启且读取成功(d,tz) | 开启但读取失败 |
|---|---|---|---|
| 无 | 无 | Available(d,tz) | Unavailable |
| Available(d,tz) | Unavailable | 相同则无，否则 Available | Unavailable |
| Unavailable | 无 | Available(d,tz) | 无 |

`Session` 删除 `context` 字段。`replace_history` 只替换 history，不再需要重置任何状态。

## Risks / Trade-offs

- [嵌套的 serde 标签行为不直观，`SystemReminder` 的单元变体用法尤其容易出错] → 为每种 `HistoryItem` 变体编写线格式快照测试和往返测试。
- [提供方可能在同一响应中返回重复的 call_id，导致工具结果配对出错] → 本次只处理空 ID；`TurnMachine` 的 ID 校验会以 `Internal` 错误暴露该问题，等真实出现后再决定去重策略。
- [`ToolCallId` 不再是 UUID，丢失了格式保证] → 生成的兜底 ID 带 `call_` 前缀，所有 ID 都视为不透明字符串，任何逻辑都不得解析它。
- [一次性修改大量公开类型，改动面大] → 按任务顺序分阶段推进，每个阶段都保证 `cargo test --workspace` 通过。
- [每次变化都发送完整快照，会多消耗约十余个 token] → 时区很少变化，日期每天最多变化一次，开销可以忽略。

## Migration Plan

没有持久化数据，也没有外部客户端，直接在一个变更中完成：

1. 先在 protocol 中完成模块重组，此时 core 只调整 `use` 路径。
2. 依次统一 ToolCallId、ToolResult、TokenUsage 和 history 条目，同步更新 core 与 genai。
3. 最后更新文档（`docs/architecture.md`、`AGENTS.md`）。

如需回滚，直接 revert 整个变更即可。
