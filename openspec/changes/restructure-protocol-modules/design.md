# Design

## Context

当前协议 crate 的布局：

```text
src/
  ids.rs  common.rs  tool.rs  history.rs  model.rs  react.rs  error.rs
  client/{mod.rs, event.rs, submission.rs}
```

`react.rs` 中的 `TurnStep` 和 `ModelOutputEffect` 只被 `tapestry-core` 的 `turn.rs`（`TurnMachine`）和 `agent.rs` 使用。`ThreadSettings` 定义在 `client/event.rs` 中，但实际只出现在提交里。线程工作任务分别处理 `Op::Configure` 和 `Op::ThreadSettings`：前者整体覆盖 `model` 和 `system_prompt`，并且每次都会发出 `SessionConfigured`；后者是局部更新，只在首次调用时发出确认。

动机见 proposal.md 的 Why 部分。

## Goals / Non-Goals

**Goals:**
- 协议 crate 只保留跨组件的数据契约，布局扁平：`common/{mod.rs, ids.rs}`、`tool`、`history`、`model`、`event`、`submission`、`error`。
- 客户端只有一个配置入口。

**Non-Goals:**
- 不调整事件类型、事件信封或事件通道。这些属于后续的“事件溯源与 EventStore” change。
- 不处理 `history.rs` 的拆分或删除，这也属于后续 change。
- 不新增“首次设置必须包含 model”的校验，沿用当前 `ThreadSettings` 的默认值行为。

## Decisions

### D1：`ids` 作为 `common` 的子模块，而不是合并进 `common.rs`

结构为 `common/mod.rs` 加 `common/ids.rs`。`mod.rs` 中 `pub use ids::*`，crate 根部继续 `pub use common::*`。`id_type!` 宏仍然只在 `ids.rs` 内部可见。

- 备选方案是把 ids 的内容直接并入 `common.rs`。这样文件会超过 300 行，ID 相关代码与 `TokenUsage`/`FinishReason` 混在一起，查找不便，因此不采用。

### D2：`react` 移入 core 的 `turn` 模块

`TurnStep` 和 `ModelOutputEffect` 放到 `tapestry-core/src/turn.rs`（或其子模块 `turn/step.rs`），与使用它们的 `TurnMachine` 放在一起。`tapestry-core` 的 `lib.rs` 继续导出这两个类型，因此下游从 `tapestry_core` 导入的方式不变。

- 保留 serde derive 和现有的往返测试。后续 change 可能需要记录回合的状态流转，因此先保留序列化能力；如果最终确认不需要，再删除。
- `TurnStep::CallModel.reminder` 引用的 `SystemReminder` 仍来自协议 crate 的 `history`。core 依赖 protocol，方向正确。

### D3：命名采用 `Submission { id, command: Command }`

- 采用 `Command`，是因为它与 `Event` 是 CQRS 中的一对概念：命令表示意图，可能被拒绝；事件表示已经发生的事实。这为后续的事件溯源做准备。
- `Submission` 保留，它是携带关联 ID 的信封，名字本身没有问题。
- 不采用的备选：`Request` 偏向同步 RPC 语义；`Action` 容易与工具动作混淆；`Operation` 只是 `Op` 的全称，问题仍然存在。
- 变体为 `UpdateSettings { settings: ThreadSettings }`、`StartTurn { content, name, turn_id }`、`Interrupt`。`StartTurn` 保持原名：它的语义准确，并且后续 change 会新增 `UserMessage` 事件，命令名和事件名需要区分开。

### D4：删除 `Configure`，统一使用 `UpdateSettings`

`ThreadSettings` 已经能完整表达 `Configure` 的两个字段。合并后，配置只走一条路径（`ThreadConfig::apply_settings`），首次确认的语义也只有一种。

- 行为差异：过去 `Configure { system_prompt: None }` 会清空系统提示词；局部更新无法表达“清空”。目前没有调用方依赖这个行为，因此不额外提供清空能力，需要时再通过显式字段实现。
- 过去重复执行 `Configure` 会每次都发出 `SessionConfigured`；合并后只在首次发出。

### D5：`ThreadSettings` 移到 `submission.rs`

`ThreadSettings` 只作为命令载荷使用，放在 submission 模块与它的用途一致。由于类型名和 crate 根部重导出都不变，`tapestry-core/config.rs` 不需要修改导入。

## Risks / Trade-offs

- [线格式破坏性变更：`op`→`command`，`configure`/`thread_settings` 被删除] → 目前工作区之外没有消费者，app server 和 CLI 仍处于规划阶段。在 proposal 中标注 **BREAKING**，并新增针对旧标签解析失败的测试。
- [下游通过 `tapestry_protocol::TurnStep` 导入会编译失败] → 工作区内只有 core 使用这些类型；core 的 `lib.rs` 继续导出它们。
- [重复配置不再有确认事件，客户端用 `collect_events_for` 等待第二次设置时会一直阻塞] → 在 `docs/architecture.md` 中说明：只有首次设置有确认事件。示例和测试只对首次设置使用 `collect_events_for`。

## Migration Plan

这是单次提交内的源码级迁移，不需要数据迁移。回滚方式是直接 revert 该提交。
