# Tasks

## 1. 协议类型

- [x] 1.1 把 `EnvironmentContext`、`SystemReminder` 移到 `common`，把 `HistoryItem` 移到 `model.rs`，删除 `history.rs`，并迁移其往返测试；更新 `lib.rs` 的重导出和分层说明；`cargo test -p tapestry-protocol` 通过
- [x] 1.2 `Event` 增加 `log_position`、`thread_id`、`turn_id`（可选）、`timestamp_ms`；从各事件载荷中删除 `turn_id`；新增往返测试，验证回合事件的信封包含 `turn_id`、非回合事件省略它、`msg` 中没有 `turn_id`；`cargo test -p tapestry-protocol` 通过
- [x] 1.3 新增 `UserMessage`、`AssistantMessage`、`EnvironmentContextUpdated`、`SystemReminder` 事件，删除 `AgentMessage`；为每个新事件编写往返测试和 snake_case 标签断言；`cargo test -p tapestry-protocol` 通过

## 2. 存储与投影（core）

- [x] 2.1 `Clock` 增加 `unix_millis()`，`SystemClock` 和 `FixedClock` 分别实现；单测验证 `FixedClock` 返回设定值
- [x] 2.2 新增 `store` 模块：`NewEvent`、`EventStore` trait、`EventSubscription`，以及 `CoreError::EventStore`；在 `lib.rs` 中重导出
- [x] 2.3 实现 `InMemoryEventStore`；单测覆盖 event-store spec 场景：连续编号、线程之间互不影响、从中间读取、多个订阅者各自收到全部事件、晚订阅者回放、慢订阅者超过 64 条事件不丢；`cargo test -p tapestry-core store::` 通过
- [x] 2.4 新增 `projection` 模块 `HistoryProjection`（`apply`/`fold`/`history`），并删除或内联 core 的 `history.rs`（`fill_missing_tool_results` 移入投影）；单测覆盖：各事件的映射、`SystemReminder` 不进入投影、回合中途 `Error` 修复、日志截断修复、`TurnAborted` 修复；`cargo test -p tapestry-core projection::` 通过

## 3. 接入回合与线程

- [x] 3.1 `TurnMachine` 不再持有 `&mut Vec<HistoryItem>`，改为返回待记录的内容；更新 `turn.rs` 的单测；`cargo test -p tapestry-core turn::` 通过
- [x] 3.2 `AgentLoop` 引入 `Recorder`，所有事件都经过 `record`（追加 + 投影）；按新顺序发出 `TurnStarted`、`EnvironmentContextUpdated`、`UserMessage`、`ModelRequestStarted`、`SystemReminder`、`AssistantMessage` 等事件；更新 `agent.rs` 的测试，断言 thread-events spec 中“含一次工具调用的回合”的完整顺序和 `log_position` 递增、强制收尾时的 `SystemReminder` 和 `AssistantMessage` 内容；`cargo test -p tapestry-core agent::` 通过
- [x] 3.3 worker 改用 store 和投影，删除 `Session.history` 和 `replace_history`；`Error` 带上当前回合的 `turn_id`；追加失败时记录日志并结束当前提交；测试使用一个会失败的 store 验证回合终止，并验证由日志 fold 出的 history 合法
- [x] 3.4 `ThreadHandle`：删除 `next_event`，新增 `subscribe(after_log_position)`，把 `collect_events_for` 移到 `EventSubscription`；迁移 `thread.rs` 的所有测试；新增测试：两个订阅者同时消费一个回合，都收到完整序列
- [x] 3.5 新增 `TapestryThread::builder`（`clock`、`event_store`、`spawn`、`resume`），`spawn`/`spawn_with_clock` 改为简写；测试覆盖：恢复后继续对话（第二回合请求包含第一回合的 history，`log_position` 接续）、恢复后未配置时 `start_turn` 返回 `Error`、投影与重新 fold 的结果相等
- [x] 3.6 迁移 `examples/minimal_turn.rs` 和 genai 示例，改用订阅接口；`cargo run -p tapestry-core --example minimal_turn` 正常运行

## 4. 文档

- [x] 4.1 更新 `docs/architecture.md`：事件溯源数据流图、信封、新事件列表与顺序、EventStore 与订阅、投影修复规则、恢复语义；更新 `AGENTS.md` 中的协议与事件约定（`HistoryItem` 位置、`collect_events_for`、事件通道相关描述、ID 职责）

## 5. 集成检查

- [x] 5.1 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace` 全部通过
- [x] 5.2 `openspec validate event-sourced-thread-history --strict` 通过

## Workflow follow-up

- 必须在 `restructure-protocol-modules` 归档后再归档本 change，因为两者对 `protocol-layers` 有先后修改。
