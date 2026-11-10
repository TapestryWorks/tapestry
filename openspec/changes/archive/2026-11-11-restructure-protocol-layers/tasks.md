# Tasks

每组完成后都必须运行 `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` 并全部通过。

## 1. protocol 模块重组（不改变类型）

- [x] 1.1 按 design D1 建立 `common.rs`、`react.rs`、`model.rs` 和 `client/`（`submission.rs`、`event.rs`），把 `ToolCallStatus` 移到 `tool.rs`、`FinishReason` 移到 `common.rs`，并在 `lib.rs` 根部重导出全部公开类型。验证：core 只需调整 `use` 路径就能编译，原有测试全部通过。
- [x] 1.2 将 `ModelRequest`/`ModelResponse` 从 core 的 `model.rs` 迁入 protocol 的 `model.rs`；core 保留 `ModelClient` trait 与 `MockModelClient`，并在 `tapestry-core/src/lib.rs` 中重导出迁走的类型。验证：genai 和全部测试编译通过。
- [x] 1.3 将 `TurnStep`、`ModelOutputEffect` 从 core 的 `turn.rs` 迁入 protocol 的 `react.rs`，`TurnMachine` 留在 core。验证：`cargo test -p tapestry-core turn::` 通过。

## 2. ToolCallId 改为字符串并复用提供方 ID

- [x] 2.1 按 D2 把 `ToolCallId` 改为 `#[serde(transparent)]` 的字符串 newtype，新增 `generate()`；`ToolCall::new` 改为接收 id。在 `ids.rs` 中补充测试，覆盖透明序列化、`generate()` 非空且带 `call_` 前缀、两次生成的值不同。
- [x] 2.2 genai 映射中使用提供方的 `call_id`，ID 为空时调用 `generate()`；回传时原样使用。在 `mapping.rs` 中补充测试，覆盖“复用提供方 ID”和“空 ID 时生成”两种情况，对应 model-protocol 规格中“工具调用 ID 来自模型提供方”的场景。
- [x] 2.3 更新 Mock、各处测试和 `examples/minimal_turn.rs` 中的 `ToolCall` 构造方式。验证：`cargo run -p tapestry-core --example minimal_turn` 运行成功。

## 3. 统一 ToolResult

- [x] 3.1 在 protocol 的 `tool.rs` 中定义 `ToolResult { call_id, tool_name, output, status }` 及 `completed/failed/not_found` 构造函数，并把 `HistoryItem::ToolResult` 改为 newtype 变体。验证：线格式快照测试确认 JSON 仍是平铺的 `{"type":"tool_result",...}`，往返测试通过。
- [x] 3.2 `ToolCallEndEvent` 改为 `{ turn_id, result, duration_ms }` 并删除 `success`。补充测试，确认序列化输出不含 `success` 且往返相等（thread-events 规格中“ToolCallEnd 不再携带 success”）。
- [x] 3.3 core 中 `Tool::execute` 改为 `execute(&self, call: &ToolCall) -> ToolResult`，删除 `ToolContext` 和 core 的 `ToolResult`；更新 `EchoTool`；`ToolRegistry` 的测试改为断言 `status`。
- [x] 3.4 `TurnMachine::on_tool_outcomes` 改为接收 `Vec<ToolResult>` 并删除 `ToolOutcome`；AgentLoop 在执行后用原调用的值覆盖 `call_id`/`tool_name`。在 `agent.rs` 中补充测试：一个返回错误 ID 的工具，最终在 history 和 `ToolCallEnd` 中都使用原 ID，且回合正常完成（tool-execution 规格中“工具结果与调用对齐”）。

## 4. TokenUsage

- [x] 4.1 按 D6 在 `common.rs` 中实现 `TokenUsage`（四个 `u64` 字段、`total_tokens()`、`AddAssign`），通过私有 wire 结构序列化 total、反序列化时忽略 total。补充测试，覆盖 model-protocol 规格中“序列化包含总量”和“忽略不一致的总量”两个场景。
- [x] 4.2 `ModelResponse`、`ModelRequestCompletedEvent` 和 `TokenCountEvent` 改为使用 `usage: TokenUsage`；AgentLoop 用 `+=` 累计。在 `agent.rs` 中补充测试，覆盖 thread-events 规格中“多次调用的汇总”场景。
- [x] 4.3 genai 映射按 D6 对用量做归一化。在 `mapping.rs` 中补充测试，覆盖“总输入包含缓存读取”“提供方不报告缓存”“输出包含推理”三个场景。

## 5. 删除 UserInput

- [x] 5.1 `Op::StartTurn` 改为 `{ content, name, turn_id }`，删除 core 的 `UserInput`；AgentLoop 根据 `content`/`name` 构造 `HistoryItem::User`，`name` 的校验逻辑不变。更新 `submission.rs` 的序列化测试，以及 thread 测试与示例中的调用方式。验证：线程测试中带名称和名称非法的用例通过。

## 6. 结构化 history 条目与 prompt 渲染

- [x] 6.1 按 D4 在 `history.rs` 中新增 `EnvironmentContext`（标签 `state`）、`SystemReminder` 以及 `HistoryItem` 的对应变体，删除 protocol 的 `context.rs`（`ContextState`）。补充 5 种变体的线格式快照测试与往返测试。
- [x] 6.2 新增 core 的 `prompt.rs`（D8），把 `render_user_content` 和 `context.rs` 中的渲染函数迁入，新增 `render_system_reminder`。补充测试，覆盖完整片段、不可用片段和 ToolLimitReached 文案。
- [x] 6.3 按 D9 把 `context::next_context_entry` 改为从 history 推导基线并返回 `Option<EnvironmentContext>`，删除 `Session.context`，简化 `replace_history`。按 D9 的状态表逐格编写单元测试，并新增 time-context 规格中“替换 history”“替换为含上下文的 history”“跨日发送完整快照”三个场景的测试。
- [x] 6.4 `TurnStep::CallModel.reminder` 改为 `Option<SystemReminder>`；AgentLoop 在请求副本中追加 `HistoryItem::SystemReminder`。更新测试：收尾请求的 history 以系统提醒结尾，持久 history 中没有系统提醒。
- [x] 6.5 genai 映射和 `MockModelClient` 改用 `prompt` 模块渲染 `User`/`EnvironmentContext`/`SystemReminder`。在 mapping 中补充测试，确认两类结构化条目被映射为无名称的用户消息；在 Mock 中补充测试，确认收尾请求不会回显系统提醒。
- [x] 6.6 把 agent/thread 测试中依赖 `content.starts_with("<environment_context>")` 的断言改为匹配 `HistoryItem::EnvironmentContext` 变体。验证：`cargo test --workspace` 通过。

## 7. 文档

- [x] 7.1 更新 `docs/architecture.md`：protocol 的分层与模块映射、ToolResult/TokenUsage、结构化 history 条目、基线从 history 推导（替换原第 83 行关于持久化 ContextState 的设想），并记录 `thought_signatures` 后续事项。验证：文档中不再出现 `ContextState`、`UserInput`、`ToolOutcome`。
- [x] 7.2 更新 `AGENTS.md`：protocol 职责改为“维护所有协议，按层划分模块”；`id_type!` 只用于 UUID 类型 ID，`ToolCallId` 是不透明字符串；在 history 条目约定中补充新增变体。验证：重新阅读，确认与 design D1–D9 一致。

## 8. 集成检查

- [x] 8.1 运行 `openspec validate restructure-protocol-layers --strict` 以及 CI 的三条命令，全部通过。
- [x] 8.2 用 `rg` 检查 core 中不再定义协议层已有的类型（`struct ModelRequest`、`struct ToolResult`、`enum TurnStep`、`struct UserInput`、`ToolOutcome`、`ContextState`），对应 protocol-layers 规格中“核心 crate 不重复定义协议类型”的场景。

## Workflow follow-up

- 实现与评审完成后，归档该变更并确认主 specs 已合并新增和修改的条目。
