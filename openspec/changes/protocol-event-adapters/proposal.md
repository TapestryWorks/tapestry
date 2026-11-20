# Proposal

## Why

事件日志记录了 agent 执行的全部事实（见 `event-sourced-thread-history` 和 `streaming-assistant-output`），但不同消费方使用的协议不同：AG-UI 面向前端，以“运行”为中心推送事件流；A2A 面向其他 agent，以“任务”为中心，状态机加上 artifact。消费方需要一种统一的方式，按需挑选事件并转换成目标协议，而不是各自去 match 全部 `EventMsg`。以后还会接入更多协议，因此转换契约必须统一，具体协议的实现需要彼此隔离。

## What Changes

- 协议 crate 新增 `adapter` 模块：
  - `EventCategory`：把每个 `EventMsg` 归入且只归入一个类别（会话、生命周期、消息、工具、模型、上下文、用量、诊断）。
  - `EventFilter`：按类别组合过滤事件。
  - `EventAdapter` trait：有状态地把 Tapestry 事件逐条转换为目标协议的输出（零到多条），并提供流结束时的收尾钩子。
- 新增 crate `tapestry-adapter-agui`：把一个线程的事件转换为 AG-UI 1.0 事件。回合对应 run，助手消息对应 TEXT_MESSAGE_*，工具调用对应 TOOL_CALL_*，模型调用对应 STEP_*，其余事件可以按 `EventFilter` 选择性地作为 CUSTOM 输出。
- 新增 crate `tapestry-adapter-a2a`：把一个线程的事件转换为 A2A 1.0 的流式响应。回合对应 task，助手文本对应 artifact（增量追加，最终以完整文本覆盖），回合结束对应终态状态更新（completed、canceled 或 failed；A2A 1.0 没有 final 标记）。
- 两个 adapter crate 只依赖 `tapestry-protocol`（以及 serde/serde_json），协议线类型在各自的 crate 内自行定义，不引入第三方协议 SDK。
- 范围只包括出站方向（事件 → 协议输出），不包括入站请求到 `Command` 的转换，也不包括 HTTP/SSE/JSON-RPC 传输层。

## Capabilities

### New Capabilities

- `event-adapters`：事件分类、过滤，以及协议转换契约（protocol crate）。
- `agui-adapter`：Tapestry 事件到 AG-UI 事件的映射规则。
- `a2a-adapter`：Tapestry 事件到 A2A 流式响应的映射规则。

### Modified Capabilities

（无）

## Impact

- 代码：`tapestry-protocol/src/adapter.rs`、`lib.rs`；新增 `crates/tapestry-adapter-agui`、`crates/tapestry-adapter-a2a`，并注册为根 `Cargo.toml` 的工作区成员。
- API：protocol 只做新增，不破坏现有 API。
- 依赖：两个新 crate 使用工作区已有的 `serde`、`serde_json`，不新增外部依赖。
- 前置：`event-sourced-thread-history`、`streaming-assistant-output`。
