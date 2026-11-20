# Proposal

## Why

协议 crate 目前混放了两类内容：一类是真正跨组件的数据契约，另一类是只在核心内部使用的类型（例如 ReAct 驱动器与回合状态机之间的 `TurnStep`）。客户端输入使用的 `Op` 命名过于宽泛，`Configure` 与 `ThreadSettings` 两个操作的语义也互相重叠。后续计划把事件作为事实来源，并为事件提供多协议 adapter（A2A、AG-UI 等），因此需要先把模块边界和命名整理清楚，作为后续 change 的基础。

本 change 是系列工作的第一步，后续依次为：事件溯源与 EventStore、流式 delta、协议 adapter。

## What Changes

- `ids` 模块并入 `common`，作为 `common::ids` 子模块；crate 根部的重导出保持不变。
- `react` 模块（`TurnStep`、`ModelOutputEffect`）从协议 crate 移到 `tapestry-core`。它们只是驱动器与状态机之间的内部消息，不再属于公开协议。**BREAKING**：`tapestry_protocol::{TurnStep, ModelOutputEffect}` 删除，改为由 `tapestry_core` 导出。
- 取消 `client` 目录：`client/event.rs` 移到 `src/event.rs`，`client/submission.rs` 移到 `src/submission.rs`。`ThreadSettings` 从 event 模块移到 submission 模块。
- **BREAKING**：`Op` 重命名为 `Command`，`Submission.op` 重命名为 `Submission.command`。`ThreadHandle::submit` 改为接收 `Command`。
- **BREAKING**：删除 `Op::Configure`，`Op::ThreadSettings` 改名为 `Command::UpdateSettings`。所有配置统一通过局部更新的 `ThreadSettings` 完成：首次 `UpdateSettings` 完成线程配置并发送 `SessionConfigured`，与现有的 `ThreadSettings` 行为一致。线格式中的标签从 `thread_settings` 变为 `update_settings`，`configure` 不再可用。
- 同步更新 `docs/architecture.md`、`AGENTS.md`、`README.md` 和示例代码。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `protocol-layers`：ReAct 内部消息不再由协议 crate 维护；模块分层改为 `common`（含 ids）← `tool` ← `history` ← `model`，`event`/`submission` 位于 crate 根部。
- `thread-events`：客户端操作改名为 `Command`；删除 `Configure`；设置操作改名为 `UpdateSettings`，并作为唯一的配置入口。

## Impact

- 代码：`crates/tapestry-protocol/src` 下的全部模块布局；`tapestry-core` 中的 `thread.rs`、`agent.rs`、`turn.rs`、`lib.rs`、`examples/*`。
- API：下游通过 `tapestry_protocol::Op` 或 `TurnStep` 使用的代码需要迁移；按 crate 根部导入的其他类型不受影响。
- 线格式：`Submission` 的 JSON 由 `{"id", "op": {"type": "configure" | "thread_settings" | ...}}` 变为 `{"id", "command": {"type": "update_settings" | "start_turn" | "interrupt"}}`。事件的线格式不变。
- 依赖：无新增依赖。
