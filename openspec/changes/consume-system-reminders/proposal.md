# Proposal

## Why

系统提醒现在只能通过 `TurnStep::CallModel.reminder` 一个字段传给模型，每次请求最多一条，并且只能在 `prepare_request` 规定的固定位置写入。提醒的过期规则是“出现在最后一条用户、助手或工具结果之前就过期”，所以写入时机稍有偏差，提醒就会失效；模型输出为空时，同一条提醒又会被重复发送。中间件拿不到 `Recorder`，只能借修改调度计划间接插入提醒；同时 `before_model` / `after_model` 可以任意改写请求和响应，绕过了事件日志。

这次把系统提醒改为“写入即为事件，由下一次模型请求消费”的统一模型：可以在任意安全时机写入多条提醒，送达规则只由事件日志推导，中间件影响模型上下文的唯一方式就是写入提醒。

## What Changes

- 新增受限的提醒写入接口：Agent 默认调度和所有中间件钩子都可以把 `SystemReminder` 加入当前回合的提醒队列，由 core 在安全点统一记录为 `SystemReminder` 事件。钩子仍然拿不到 `Recorder`，也不能写入其他事件。
- 提醒的消费与过期规则改由投影推导：提醒在其后第一个 `ModelRequestCompleted` 出现时视为已消费；遇到 `TurnStarted` 时，上一回合未消费的提醒全部失效。已消费或已失效的提醒不进入投影。有效提醒不论在日志中的位置，都统一放在投影末尾。
- **BREAKING** 删除 `TurnStep::CallModel.reminder`；默认的超限收尾改为通过提醒队列写入 `tool_limit_reached`。
- **BREAKING** 删除 `SystemReminderEvent.iteration`，线格式变为 `{"type":"system_reminder","reminder":{"reminder":...}}`；旧日志仍可解析。
- **BREAKING** `Middleware::before_model` 改为接收只读的事件快照，不再接收可改写的 `ModelRequest`；`Middleware::after_model` 改为接收只读的 `ModelResponse`。删除 `validate_model_request`。`before_tool` / `after_tool` 不变。
- 删除 `Agent::prepare_request`，把请求组装展开到 `Agent::reasoning_with_hooks`。
- **BREAKING** 重命名：`HistoryProjection` 改为 `ConversationProjection`，`history()` 改为 `messages()`，协议类型 `HistoryItem` 改为 `ConversationItem`（serde 线格式不变）。默认 formatter 删除基于位置的提醒过滤，直接使用投影结果。
- 同步更新 adapter 测试、`docs/architecture.md`、AGENTS 说明，以及 `SystemReminder` 中过时的 “never persisted” 注释。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `conversation-history`：系统提醒的投影、消费和过期规则，有效提醒放在末尾；投影与条目改名。
- `thread-events`：`SystemReminder` 事件不再携带迭代序号，并可以在下一次模型请求前的任意安全点记录。
- `react-turn-loop`：超限收尾提醒的写入方式及其在后续请求和回合中的可见性。
- `model-protocol`：模型请求快照中提醒的送达语义。
- `agent-middleware`：模型前后钩子改为只读；新增受限的提醒写入能力。
- `agent-turn-policy`：调用模型的步骤不再携带提醒，Agent 改为通过提醒队列写入。

## Impact

- `tapestry-protocol`：`SystemReminderEvent`、`HistoryItem` 改名为 `ConversationItem`，以及相应的 re-export 和 serde 测试。
- `tapestry-core`：`agent/agent_trait.rs`（`TurnStep`、`TurnContext`、`reasoning_with_hooks`、默认 `forward`）、`middleware`（`HookCtx`、`Middleware` 签名）、`formatter/formatter_trait.rs`（投影与过滤规则）、`recorder.rs`、`projection.rs`、`thread.rs`、`agent/agent_loop.rs`，以及 genai 映射和 mock 模型的测试。
- `tapestry-adapter-agui`、`tapestry-adapter-a2a`、`tapestry-adapter-cli`：`SystemReminder` 事件构造方式变化，需要更新测试。
- 下游的中间件和自定义 Agent：需要迁移 `before_model` / `after_model` 签名和 `TurnStep::CallModel` 的构造。
- 文档：`docs/architecture.md`、AGENTS 说明。
