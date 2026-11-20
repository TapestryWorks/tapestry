# Proposal

## Why

现在线程的 history 和事件流是两套互相独立的记录：history 只保存在 worker 内存里，事件经过 mpsc 通道发出后就被消费掉了。事件流也不完整，没有用户输入、环境上下文、系统提醒，助手的工具调用也不完整，因此消费方无法从事件还原 agent 的完整执行过程。另外，mpsc 只有一个接收端，多个消费者（例如以后同时接入 AG-UI 和 A2A）会互相抢事件。

本 change 让**事件日志成为唯一的事实来源**：所有执行过程都记录为有序事件，history 是事件的投影，消费者通过各自的游标订阅日志。它依赖 `restructure-protocol-modules`，并为后续的流式输出和协议 adapter 打基础。

## What Changes

- **事件信封**：`Event` 增加 `log_position`（线程内单调递增，从 1 开始）、`thread_id`、可选的 `turn_id` 和 `timestamp_ms`。**BREAKING**：各事件载荷中的 `turn_id` 字段统一上移到信封。
- **补全事件**，使 history 可以完全由事件推导：
  - 新增 `UserMessage`、`EnvironmentContextUpdated`、`SystemReminder`。
  - **BREAKING**：`AgentMessage` 被 `AssistantMessage` 取代，后者携带文本和工具调用，每次产生 history 助手条目时都发出一条。
- **history 成为投影**：core 提供 fold 规则（事件 → `HistoryItem`）。worker 只能通过“追加事件”来改变 history；回合异常结束时，补齐缺失工具结果的修复也由投影规则完成。
- **协议模块调整**：删除 `history.rs`。`HistoryItem` 移入 `model.rs`，因为它是模型请求的组成部分；`EnvironmentContext`、`SystemReminder` 移入 `common`，供事件层和模型层共用。
- **EventStore**：在 core 中定义 `EventStore` trait，提供追加、按序读取、从指定日志位置之后订阅三种能力，并实现内存后端，作为默认后端。
- **订阅替代 mpsc 事件通道**。**BREAKING**：`ThreadHandle::next_event` 被 `ThreadHandle::subscribe(after_log_position)` 取代。每个订阅者有独立的游标，可以先回放历史再接收实时事件，不会丢失也不会互相抢夺；`collect_events_for` 改为订阅对象上的方法。提交通道仍然使用 mpsc。
- **恢复线程**：可以基于已有的 EventStore 和线程 ID 恢复线程，fold 出 history 后继续执行回合。线程配置不持久化，恢复后仍需先发送 `UpdateSettings`。

## Capabilities

### New Capabilities

- `event-store`：事件日志的存储契约（追加、顺序、读取、订阅、多后端可替换）、内存后端，以及从日志恢复线程。

### Modified Capabilities

- `thread-events`：事件信封字段；成功回合的事件顺序加入新事件；`AgentMessage` 由 `AssistantMessage` 取代；新增 `UserMessage`、`EnvironmentContextUpdated`、`SystemReminder` 事件；`turn_id` 上移到信封。
- `conversation-history`：history 由事件日志投影得到，跨回合保留的语义改为基于日志。
- `protocol-layers`：删除 history 层；环境上下文和系统提醒归入公共层，history 条目归入模型层。
- `react-turn-loop`：基础设施错误改为“事件日志追加失败”，不再是“事件通道关闭”；history 修复由投影完成。

## Impact

- 代码：`tapestry-protocol` 的 `event.rs`、`model.rs`、`common/`，删除 `history.rs`；`tapestry-core` 的 `thread.rs`、`agent.rs`、`turn.rs`、`history.rs`、`context.rs`、`clock.rs`、`lib.rs`、`examples/*`，并新增 `store` 模块和 `projection` 模块。
- API：`ThreadHandle` 的事件消费接口；`TapestryThread` 的构造方式（可以注入 EventStore）；`Clock` 增加获取 Unix 毫秒时间的接口。
- 线格式：所有事件的 JSON 结构都会变化（信封字段、`turn_id` 位置、`agent_message` 被删除）。
- 依赖：不新增外部依赖。订阅对象使用自定义的异步 `next()`，不引入 `futures`/`tokio-stream`。
