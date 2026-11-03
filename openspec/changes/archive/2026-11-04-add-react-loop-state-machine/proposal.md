# Proposal

## Why

当前 `AgentLoop::run_turn` 把决策、模型调用、工具执行和事件发送混在一个函数里，且存在几个影响 ReAct 正确性的缺陷：模型请求只携带最后一条用户消息和工具结果（没有真实 history，无法多轮对话）；未知工具或工具执行失败会直接中断整个回合，而不是作为 observation 回灌给模型；`max_tool_iterations` 耗尽只能报错；终态事件缺少结构化的结束原因。同时项目要求跨平台编译，需要用 CI 守住依赖边界。框架尚未上线，现在是做破坏性重构的最佳时机。

## What Changes

- 将 ReAct 循环重构为“纯同步状态机 `TurnMachine`（只负责决策）+ 异步驱动器 `AgentLoop`（负责 IO 与事件发射）”。
- 引入 `History`/`HistoryItem`（`User`、`Assistant`、`ToolResult`），由线程会话持有并跨回合保留；模型请求改为基于 history。
- `User` 条目带可选 `name`，用于标识多 agent 场景下的发言者；模型适配层渲染为 `"name: content"`。`Assistant` 条目不带 `name`。
- 自身名字通过 `agent_name` 配置注入 system prompt，而非写入助手消息。
- 工具增加 `ToolSpec`（JSON Schema 描述）；`Tool::execute` 不再返回 `Err`，失败以 `success: false` 表达。未知工具、执行失败均作为 `ToolResult` 回灌，循环继续。
- 超过 `max_tool_iterations` 时，追加一次不带工具的“强制收尾”模型调用（临时 reminder，不写入持久 history），以 `finish_reason: exceed_max_iters` 正常结束，而非 `Error`。
- 协议新增/变更事件：
  - 新增 `ModelRequestStarted`、`ModelRequestCompleted`、`Warning`。
  - `ToolCallEnd` 增加 `status`、`duration_ms`。
  - `TurnComplete` 增加 `last_agent_message`、`finish_reason`。
  - **BREAKING**：`TurnAborted.reason` 由字符串改为枚举 `TurnAbortReason`，并增加可选 `message`。
  - `Op::StartTurn` 增加 `user_name`；`ThreadSettings` 增加 `agent_name`。
  - 保留 `AgentMessage`，语义明确为“一条完整的助手消息”。
- **BREAKING**：`Tool` trait、`ModelClient`/`ModelRequest`/`ModelResponse` 签名变化，`MockModelClient`、`EchoTool`、genai 映射、示例与测试同步更新。
- CI 增加 ubuntu/macOS/windows 的 `cargo check` 矩阵（含 `--features genai`），并对 `tapestry-protocol` 增加 `wasm32-unknown-unknown` 检查；处理 `uuid` 在 wasm32 上的随机源依赖。
- 更新 `docs/architecture.md`（状态机、事件顺序、P1 限制）。

不在本次范围：流式输出、回合内中断（`Interrupt` 仍为串行处理）、并行工具、hook、审批、history 压缩、多 agent 间通信与连续 user 消息折叠。

## Capabilities

### New Capabilities
- `react-turn-loop`: 回合内“模型 → 工具 → 模型”的状态机决策、终止条件、错误回灌、超限强制收尾与结束原因。
- `conversation-history`: 线程级 history 的结构、跨回合保留、`User.name` 渲染规则与 `agent_name` 注入 system prompt。
- `tool-execution`: 工具规格（`ToolSpec`）、不返回 `Err` 的执行契约、未知/失败/成功状态与 `ToolCallBegin/End` 配对。
- `thread-events`: 协议事件与字段（新增事件、`FinishReason`、`TurnAbortReason`）、事件顺序契约与 serde 兼容性。
- `cross-platform-build`: workspace 依赖约束与 CI 的多平台/wasm32 检查。

### Modified Capabilities

（无：`openspec/specs/` 下目前没有既有 spec。）

## Impact

- 代码：`crates/tapestry-protocol`（`event.rs`、`submission.rs`、`lib.rs`、`Cargo.toml`）；`crates/tapestry-core`（`agent.rs`、`thread.rs`、`model.rs`、`tool.rs`、`config.rs`、`genai/*`、新增状态机与 history 模块、示例与测试）。
- API：公开协议与 `Tool`/`ModelClient` trait 存在破坏性变更（项目未上线，已获允许）。
- 依赖：`tapestry-protocol` 增加 wasm32 目标专属的 `getrandom` 特性配置；不新增运行时依赖。
- 构建/CI：`.github/workflows` 增加平台矩阵与 wasm32 任务。
- 文档：`docs/architecture.md`。
