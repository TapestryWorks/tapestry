# Proposal

## Why

目前 `tapestry-protocol` 只承载面向客户端的线协议，其他协议性质的数据契约仍散落在 `tapestry-core` 中，包括调用模型用的 `ModelRequest`/`ModelResponse`、ReAct 驱动器与状态机之间的 `TurnStep`/`ToolOutcome`、工具结果 `ToolResult`，以及 `UserInput`。由此产生了几个问题：

- 同一概念存在多份表示。例如工具结果有 4 种形态，`success: bool` 与 `ToolCallStatus` 两套语义并存；token 计数字段在三处重复。
- 时间上下文和强制收尾提示被伪装成 `HistoryItem::User` 的格式化字符串，系统只能通过字符串前缀识别它们。
- 自行生成的 UUID 替代了模型提供方的原始 call_id。

项目仍处于早期，适合一次性理清分层，并接受线格式的不兼容变更。

## What Changes

- 重新定义 protocol 的职责：凡是协议类型都由 `tapestry-protocol` 唯一维护，并按层划分模块，包括公共类型、工具协议、history、模型统一协议、ReAct 内部消息和前端协议。core 只保留行为：trait、状态机、IO、策略与渲染。
- 将 `ModelRequest`/`ModelResponse` 迁入 protocol 的模型层并支持 serde。删除 `ModelRequest` 中 reminder 的旁路，强制收尾提示改由 history 条目表达。
- 新增 `TokenUsage { input_tokens, output_tokens, cache_read_tokens, cache_write_tokens }`，字段类型为 `u64`。`total_tokens` 是动态计算的派生值，序列化时输出，反序列化时忽略。`output_tokens` 包含 reasoning。genai 映射时，从总输入中扣除缓存读取和缓存写入部分。**BREAKING**：`ModelRequestCompleted` 和 `TokenCount` 改为携带 `usage`。
- 将工具结果统一为 protocol 中唯一的 `ToolResult { call_id, tool_name, output, status }`，`not_found` 也直接用它表示。删除 core 中的 `ToolResult(success)` 和 `ToolOutcome`，`HistoryItem::ToolResult` 直接包装该类型。**BREAKING**：`ToolCallEnd` 改为 `{ turn_id, result, duration_ms }`，并删除 `success`。
- `Tool::execute` 改为 `execute(&self, call: &ToolCall) -> ToolResult`，删除未被使用的 `ToolContext`。结果通过 `ToolResult::completed/failed/not_found(&call, ..)` 构造，AgentLoop 执行后以原始调用为准，修正 `call_id` 和 `tool_name`。
- **BREAKING**：`ToolCallId` 改为字符串 newtype，直接复用模型提供方返回的原始 call_id。只有提供方未给出 ID 或 ID 为空时才自动生成。
- 删除 `UserInput`，用户消息统一为 `HistoryItem::User`。**BREAKING**：`Op::StartTurn` 字段改为 `{ content, name?, turn_id? }`。
- 新增 `HistoryItem::EnvironmentContext`，变体为 `Available { date, timezone }` 或 `Unavailable`，每条都记录完整快照。已发送的基线由 history 中最后一条该条目推导，删除 `ContextState`。日期或时区任一变化时，重发完整快照（不再只发送变化字段）。
- 新增 `HistoryItem::SystemReminder`，当前取值为 `ToolLimitReached`。它只追加到单次模型请求的 history 副本中，不持久化。
- 将 `TurnStep`、`ModelOutputEffect` 迁入 protocol 的 react 层并支持 serde。`TurnStep::CallModel` 中的 reminder 改为 `Option<SystemReminder>`。
- core 新增统一的提示词渲染模块，在发送模型请求时把 `User`、`EnvironmentContext`、`SystemReminder` 渲染为文本。genai 和 Mock 客户端共用该模块，渲染格式保持 `<environment_context>` 等现有约定。
- 更新 `docs/architecture.md` 与协作说明中对 protocol 职责、ID 约定和 history 条目的描述。
- 后续事项（不在本变更范围内）：genai 映射目前丢弃 Gemini 的 `thought_signatures`，可能导致思考模型的多轮工具调用失败，需要在模型协议中补充提供方元数据。

## Capabilities

### New Capabilities
- `protocol-layers`：protocol crate 的分层职责与模块边界，规定哪些类型必须由 protocol 维护，以及各层之间允许的依赖方向。
- `model-protocol`：后端模型统一协议，涵盖 `ModelRequest`/`ModelResponse` 的结构与序列化、`TokenUsage` 语义及提供方 usage 的归一化，以及工具调用 ID 来源于提供方的规则。

### Modified Capabilities
- `conversation-history`：history 条目类型新增 `EnvironmentContext` 与 `SystemReminder`。用户消息的名称来源改为 `StartTurn.name`。
- `react-turn-loop`：强制收尾提示改为非持久化的 `SystemReminder` 条目。工具失败改用 `ToolResult.status` 描述。
- `thread-events`：`StartTurn` 字段调整；`ModelRequestCompleted`/`TokenCount` 改为携带 `TokenUsage`；`ToolCallEnd` 改为携带 `ToolResult` 并删除 `success`。
- `time-context`：上下文改为结构化 history 条目，记录完整快照；基线由 history 推导；变化时发送完整快照；history 改写后的重发规则相应调整。
- `tool-execution`：执行结果契约改为统一的 `ToolResult` 和状态枚举；工具签名改为接收 `ToolCall`；调用 ID 来自提供方。

## Impact

- `tapestry-protocol`：模块重组；新增 `model`、`react`、`tool`（扩充）、`common` 等模块；`ToolCallId` 改为字符串；删除 `ContextState`；`lib.rs` 的重导出相应调整。
- `tapestry-core`：涉及 `model.rs`、`turn.rs`、`tool.rs`、`agent.rs`、`thread.rs`、`context.rs`、`history.rs`、`genai/mapping.rs` 和 `lib.rs`，新增 `prompt` 渲染模块，并更新 `examples/minimal_turn.rs`。
- 公开线格式：`StartTurn`、`ToolCallEnd`、`ModelRequestCompleted`、`TokenCount` 和 `HistoryItem` 均有不兼容变更；不提供旧格式兼容。
- 扩展点：所有 `Tool` 实现需要改用新签名；所有 `ModelClient` 实现需要返回 `TokenUsage`，并沿用提供方的 call_id。
- 文档：`docs/architecture.md` 与 `.github` 下的协作说明。
