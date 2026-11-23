# Design

## Context

- 提醒目前的唯一入口是 `TurnStep::CallModel.reminder: Option<SystemReminder>`。`Agent::prepare_request` 先记录 `ModelRequestStarted`，再记录 `SystemReminder{iteration, reminder}`，然后读取事件快照组装 `ModelRequest`，只有 `reasoning_with_hooks` 调用它。
- `HistoryProjection`（`formatter/formatter_trait.rs`，`projection.rs` 中有兼容的 re-export）把提醒保存在 `items` 中原来的位置。`Formatter::history_items_to_messages` 只发送最后一个 User/Assistant/ToolResult 之后的提醒；`fill_missing_tool_results` 需要专门计算位置，把补全结果插到尾部提醒之前。
- `HookCtx<'a>` 只读，包含 `thread_id`、`turn_id`、`iteration` 和 `config`。`before_model(&mut ModelRequest)` 和 `after_model(&mut ModelResponse)` 可以改写数据，`validate_model_request` 用来防止重新开启工具或修改迭代序号。目前只有测试代码会改写请求或响应。
- `Recorder` 是唯一的写入入口：先追加到 store，再 `apply` 到投影。线程恢复时用 `fold` 重建投影。`agent_loop.rs` 用 `recorder.history()` 判断时间上下文基线。

## Goals / Non-Goals

**Goals:**
- 增量 `apply` 与整体 `fold` 得到相同的投影，二者都只依据事件顺序推导提醒的有效性。
- 提醒的写入时机由 core 统一控制，调用方只负责把提醒加入队列。
- 改名后的公开 API 和线格式（`ConversationItem` 的 serde tag 不变）保持清晰的边界。

**Non-Goals:**
- 不实现 prompt cache 断点（`cache_control`）。这次只保证有效提醒总在请求末尾，以后可以在最后一条非提醒消息上打断点。
- 不改变 `before_tool` / `after_tool` 的改写能力，也不改变 `after_step` 的调度能力。
- 不新增其他可由钩子写入的事件类型。

## Decisions

### 1. 投影把有效提醒单独保存，并统一放在末尾

```
ConversationProjection {
    items:     Vec<ConversationItem>,   // 不包含提醒
    reminders: Vec<SystemReminder>,     // 当前有效的提醒
    open_turn: Option<TurnId>,
}
apply(event):
    TurnStarted             → close_turn(); reminders.clear(); open_turn = ...
    ModelRequestCompleted   → reminders.clear()
    SystemReminder(r)       → reminders.push(r)
    其他事件                → 沿用现有规则写入 items
messages() = items ++ reminders.map(ConversationItem::SystemReminder)
```

- `messages()` 返回 `Vec<ConversationItem>`（或 `impl Iterator`），不再返回切片；`Recorder::history()` 相应改为 `messages()`。
- `fill_missing_tool_results` 只需在 `items` 末尾追加补全结果，不再计算提醒的位置。
- 默认 `Formatter::history_items_to_messages` 删除提醒过滤，按顺序映射每个条目。`events_to_history_items` 改名为 `events_to_items` 并返回 `messages()`。
- `ConversationItem::SystemReminder` 仍然保留，用于在消息视图中表示提醒，也为自定义 formatter 提供结构化数据。
- 备选方案：保留提醒的原始位置，只给它标记是否有效。不采用，因为提醒会夹在工具调用与工具结果之间，OpenAI 等提供方会拒绝这样的请求。

### 2. 提醒队列放在 `TurnContext` 中，由 `HookCtx` 共享

- 新增 `ReminderQueue`（core 内部类型，包装 `std::sync::Mutex<Vec<SystemReminder>>`），由 `TurnContext` 通过 `Arc` 持有。`TurnContext::remind(r)` 供 Agent 使用；`HookCtx` 持有 `Arc` 的克隆并提供 `remind(&self, r)`，因此钩子即使只有 `&HookCtx` 也能写入。
- `TurnContext::flush_reminders(&mut self) -> Result<Vec<Event>, CoreError>` 按加入顺序逐条记录 `SystemReminder` 事件，并返回 store 给出的 `Event`。
- 锁只在 `push` 和 `drain` 时短暂持有，不会跨越 `.await`。
- 备选方案：钩子返回 `Vec<SystemReminder>`。不采用，因为每个钩子的签名都要改，`before_tool` 的 `ToolDecision` 也得跟着变复杂。
- 备选方案：给钩子 `&mut Recorder`。已被 add-agent-middleware 否决。

### 3. 安全点（由 core 在这些位置调用 `flush_reminders`）

| 写入阶段 | 记录位置 |
|---|---|
| `before_turn` 及 `forward` 开始前 Agent 的写入 | `before_turn` 钩子全部执行完后 |
| 默认调度（`CallTool` 后预算耗尽）和 `after_step` | 调度节点结束后、下一步开始前 |
| `before_model` | 钩子执行完后，在本次请求快照组装前 |
| `after_model` | 助手消息事件记录完后（避免插入 Start/Delta 与 `AssistantMessage` 之间） |
| `before_tool` | 该工具的 `ToolCallBegin` 记录之前 |
| `after_tool` | 该工具的 `ToolCallEnd` 记录之后 |
| `after_turn` 及兜底 | 记录 `TokenCount` 之前 |

此外，在每次组装模型请求前都会 flush 一次，确保队列中剩下的提醒都进入本次快照。

### 4. 展开后的 `reasoning_with_hooks`

```
record ModelRequestStarted{iteration, allow_tools}
flush_reminders()                       // 调度阶段遗留的提醒
events = recorder.events()              // 快照
before_model(&hook_ctx, &events)        // 只读
events.extend(flush_reminders()?)       // 与日志前缀保持一致，无需重新读取 store
request = ModelRequest{system_prompt, events, tools(allow_tools), iteration}
output = self.reasoning(ctx, request)
after_model(&hook_ctx, &output.response) // 只读
```

- 提醒总在 `ModelRequestStarted` 之后、`ModelRequestCompleted` 之前记录，因此一定被本次请求消费。调度阶段的提醒也可以在 `ModelRequestStarted` 之前记录，按第 3 节的时机执行即可，两种位置的消费结果相同。
- 删除 `Agent::prepare_request` 和 `validate_model_request`。工具规格只由 `allow_tools` 决定。

### 5. 默认调度改用队列

- 删除 `TurnStep::CallModel.reminder`。`initial_step` 和 `CallTool` 分支在预算耗尽时调用 `ctx.remind(SystemReminder::ToolLimitReached)`，并返回 `CallModel { allow_tools: false }`。
- `initial_step` 目前是纯函数。改为由 `forward_with_hooks` 根据初始计划写入提醒：如果初始计划是禁工具调用且由预算导致，在 `before_turn` 之前加入提醒。这样回合前钩子把计划改成 `Done` 时，提醒仍会记录，但会在下一个 `TurnStarted` 失效，不会产生实际影响。

### 6. 协议层变更

- `SystemReminderEvent` 删除 `iteration`，只保留 `reminder`，线格式变为 `{"type":"system_reminder","reminder":{"reminder":...}}`。协议层没有使用 `deny_unknown_fields`，旧日志中的 `iteration` 在反序列化时会被忽略，需要增加测试确认。
- `HistoryItem` 改名为 `ConversationItem`，`HistoryItem::system_reminder` 等构造函数同步改名，`tapestry-protocol` 和 `tapestry-core` 的 re-export 同步更新。
- 更新 `SystemReminder` 的注释：提醒会被持久化，并由下一次完成的模型请求消费。

## Risks / Trade-offs

- [删除 `before_model` / `after_model` 的改写能力是破坏性变更] → 仓库内只有测试依赖这些能力，测试改为验证只读观察和提醒写入。下游迁移方式写在 spec 的 REMOVED 说明中。
- [增量 `apply` 与 `fold` 不一致] → 两者共用同一个 `apply`，并增加 fold 与实时投影相等的测试，覆盖提醒消费、跨回合失效和补全的组合。
- [请求失败时，`ModelRequestStarted` 之后记录的提醒没有被消费] → 下一回合开始时由 `TurnStarted` 统一失效；同一回合内不会重试请求，所以不会重复发送。
- [消息视图总以提醒结尾，可能被误认为是用户的新输入] → 提醒以现有渲染格式发送，模型侧语义不变。
- [`HookCtx` 新增 `Arc` 字段，测试中直接构造 `HookCtx` 的地方需要修改] → 提供 `HookCtx` 的测试构造函数，或沿用 `TurnContext::hook_ctx`。

## Migration Plan

这是单仓库内的破坏性 API 变更，不涉及数据迁移。已有事件日志中的 `system_reminder` 带有 `iteration` 字段，反序列化时会被忽略；重新 fold 后按新规则得到投影。回滚方式为 revert 该 change。
