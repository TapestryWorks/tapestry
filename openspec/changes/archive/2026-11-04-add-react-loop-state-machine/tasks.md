# Tasks

## 1. 协议层（tapestry-protocol）

- [x] 1.1 新增 `FinishReason`、`TurnAbortReason`、`ToolCallStatus` 枚举，以及 `ModelRequestStartedEvent`、`ModelRequestCompletedEvent`、`WarningEvent`，加入 `EventMsg` 并在 `lib.rs` 重导出；验证：新增 serde 往返测试通过（`cargo test -p tapestry-protocol`）
- [x] 1.2 扩展 `ToolCallEndEvent`（`status`、`duration_ms`）、`TurnCompleteEvent`（`last_agent_message`、`finish_reason`），`TurnAbortedEvent.reason` 改为枚举并增加可选 `message`；验证：往返测试与“缺失新字段可反序列化”测试通过
- [x] 1.3 `Op::StartTurn` 增加 `user_name`，`ThreadSettings` 增加 `agent_name`（均 `serde(default)` + `skip_serializing_if`）；验证：省略/缺失字段的序列化与反序列化测试通过
- [x] 1.4 在 `tapestry-protocol/Cargo.toml` 增加 wasm32 目标专属的 `getrandom` 特性配置；用 `cargo tree -p tapestry-protocol` 确认版本与 `uuid` 一致；验证：`rustup target add wasm32-unknown-unknown && cargo check -p tapestry-protocol --target wasm32-unknown-unknown` 成功

## 2. 核心基础类型（tapestry-core）

- [x] 2.1 新增 history 模块：`HistoryItem`（`User{name}`、`Assistant`、`ToolResult`）、`ToolCall`（含 `ToolCallId`）；验证：单元测试覆盖条目构造与顺序
- [x] 2.2 实现 `render_user_content(name, content)`；验证：测试覆盖有名称、无名称、空内容
- [x] 2.3 重构 `Tool`：新增 `ToolSpec` 与 `spec()`，`execute` 返回 `ToolResult`（无 `Err`），`ToolRegistry::specs()`；更新 `EchoTool`；验证：注册表与 `EchoTool` 单测通过
- [x] 2.4 重构 `ModelRequest{system_prompt, history, tools, iteration}` 与 `ModelResponse`（工具调用带 ID）；`ThreadConfig` 增加 `agent_name` 与 `apply_settings` 支持；实现 system prompt 的 `agent_name` 追加逻辑；验证：配置合并与 system prompt 注入（有/无 `agent_name`）单测通过
- [x] 2.5 更新 `MockModelClient` 以基于 history 工作并使用 `render_user_content`；验证：mock 的单测（直接回答、`tool:echo` 路径、带名称用户消息）通过

## 3. 状态机

- [x] 3.1 实现 `TurnMachine`（`next_step`、`on_model_output`、`on_tool_outcomes`），覆盖完成、工具循环、顺序写入 history；验证：纯同步单测通过
- [x] 3.2 实现未知工具/失败回灌与超限强制收尾（`allow_tools=false`、临时 reminder 不入 history、收尾仍请求工具则忽略并标记 Warning）；验证：单测覆盖 `not_found`、`failed`、超限、收尾仍请求工具、reminder 不入 history
- [x] 3.3 实现模型失败时为未完成工具调用补 `failed` 结果的 history 修复逻辑；验证：单测断言修复后每个工具调用都有对应结果

## 4. 驱动器与线程

- [x] 4.1 重写 `AgentLoop::run_turn` 为驱动器：按步骤调用模型/工具，发出 `ModelRequestStarted/Completed`、`AgentMessage`、成对的 `ToolCallBegin/End`（含 `status`、`duration_ms`）、`Warning`、`TokenCount`、`TurnComplete`；验证：事件顺序契约测试通过（含未知工具、超限、通道关闭）
- [x] 4.2 线程会话持有 history 并跨回合保留；`Op::StartTurn` 处理 `user_name` 校验（非空、无换行，违反时发 `Error` 且 history 不变）；验证：多回合上下文可见与非法名称测试通过
- [x] 4.3 `Op::Interrupt` 使用 `TurnAbortReason::Interrupted`；`ThreadSettings.agent_name` 局部更新；验证：线程测试覆盖 Interrupt 原因枚举与 `agent_name` 更新、`collect_events_for` 仍能匹配终止事件

## 5. genai 适配、示例与回归

- [x] 5.1 更新 `genai` 映射：history → genai 消息（用户名前缀、助手工具调用、工具结果）、工具规格 → genai tools、用量映射；验证：`cargo test -p tapestry-core --features genai` 中的映射测试通过
- [x] 5.2 更新示例与既有测试（`minimal_turn`、`genai_multi_provider`、`tests/genai_integration.rs`）以适配新 API；验证：`cargo run -p tapestry-core --example minimal_turn` 输出符合预期，且 `cargo check --workspace --all-targets --features tapestry-core/genai` 成功

## 6. CI 与文档

- [x] 6.1 更新 `.github/workflows` CI：ubuntu/macos/windows 的 `cargo check --workspace` 与 `-p tapestry-core --features genai`，新增 `tapestry-protocol` 的 wasm32 任务，保留 fmt/clippy/test；验证：工作流 YAML 通过 `actionlint`（或在 PR 上运行成功）
- [x] 6.2 更新 `docs/architecture.md`：状态机与驱动器、history 与 `name` 渲染、事件顺序契约、P1 限制（无回合内中断、history 无上限）、跨平台守则与后续阶段；验证：文档内容与 specs 一致并经人工检查
- [x] 6.3 全量门禁：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace` 全部通过
