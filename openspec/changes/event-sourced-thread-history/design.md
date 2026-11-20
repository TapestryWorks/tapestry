# Design

## Context

本 change 建立在 `restructure-protocol-modules` 完成后的布局之上：`common/{mod,ids}.rs`、`tool`、`history`、`model`、`event`、`submission`。

core 现状：
- `Session.history: Vec<HistoryItem>` 由 worker 持有。`TurnMachine` 持有 `&mut Vec<HistoryItem>`，在模型输出、工具结果到达时直接 push。回合出错时，`repair_history()` 调用 `fill_missing_tool_results` 补齐失败结果。
- `AgentLoop::run_turn` 先 push 环境上下文条目和用户消息，再通过 `Emitter` 向 `mpsc::Sender<Event>` 发送事件。
- `ThreadHandle` 持有 `Arc<Mutex<mpsc::Receiver<Event>>>`，所有 clone 共用同一个接收端。
- `next_context_entry` 只依据 history 推导时间上下文基线，因此 history 只要能由事件推导，时间上下文规则就不受影响。

## Goals / Non-Goals

**Goals:**
- 写入路径唯一：`record(event)` = 追加到存储 → 应用到 history 投影。
- 订阅不丢事件、不抢事件，并支持回放。
- `EventStore` 契约足以支撑后续的 SQLite、Postgres 等后端，但本次只实现内存后端。

**Non-Goals:**
- 不持久化、不恢复线程配置（`ThreadSettings`）。
- 不支持 history 压缩或整体替换（`replace_history` 目前是死代码，本次删除）。压缩需要快照事件，留到以后。
- 不实现流式 delta，也不实现协议 adapter（由后续 change 完成）。
- 不提供事件保留期或裁剪策略。

## Decisions

### D1：信封字段

```rust
pub struct Event {
    pub id: SubmissionId,          // 保留原字段名，减少破坏
    pub log_position: u64,         // 线程内日志位置，从 1 开始
    pub thread_id: ThreadId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub timestamp_ms: u64,         // Unix epoch 毫秒
    pub msg: EventMsg,
}
```

- `turn_id` 上移到信封：adapter 和过滤器不需要逐个变体 match 就能按回合分组。`TurnAborted` 当前的 `turn_id: Option` 也一并移除。
- 时间戳使用 `u64` 毫秒，protocol 不需要引入 chrono。core 的 `Clock` trait 增加 `fn unix_millis(&self) -> u64`：`SystemClock` 读取 `SystemTime`，`FixedClock` 返回固定值，便于测试断言。
- 备选方案是把 `id` 重命名为 `submission_id`，语义更清楚，但会进一步扩大破坏范围，因此不采用。

### D2：`log_position` 由存储分配

core 内部构造 `NewEvent { id, turn_id, timestamp_ms, msg }`，然后调用 `EventStore::append(thread_id, NewEvent) -> Result<Event, CoreError>`，由存储返回带 `log_position` 的完整事件。这样只有存储一个地方决定顺序，数据库后端可以直接使用自增序列或事务。

### D3：`EventStore` trait（放在 core 的 `store` 模块）

```rust
#[async_trait]
pub trait EventStore: Send + Sync {
    async fn append(&self, thread: ThreadId, event: NewEvent) -> Result<Event, CoreError>;
    async fn read_after(&self, thread: ThreadId, after_log_position: u64) -> Result<Vec<Event>, CoreError>;
    async fn subscribe(&self, thread: ThreadId, after_log_position: u64) -> Result<EventSubscription, CoreError>;
}
```

- `EventSubscription` 是 core 中的具体类型，提供 `async fn next(&mut self) -> Option<Result<Event, CoreError>>`，内部实现由后端决定（`Box<dyn SubscriptionSource>`）。不引入 `futures::Stream`，以免增加依赖；以后需要时可以再加 `impl Stream`。
- 内存后端 `InMemoryEventStore`：`RwLock<HashMap<ThreadId, ThreadLog>>`，其中 `ThreadLog { events: Vec<Event>, notify: watch::Sender<u64> }`。订阅者维护自己的游标：先在存储中读取游标之后的事件，读完后等待 `watch` 变化再继续读。由于事件存在 `Vec` 中，而不是通道缓冲区里，慢消费者不会丢事件。
- 备选方案是 `tokio::sync::broadcast`：消费慢时会返回 `Lagged` 并丢失事件，也无法回放历史，与“事件是事实来源”冲突，因此不采用。
- 新增 `CoreError::EventStore(String)`，表示追加或读取失败。`ChannelClosed` 只用于提交通道。

### D4：history 投影

core 新增 `projection` 模块：

```rust
pub struct HistoryProjection { items: Vec<HistoryItem>, open_turn: Option<TurnId> }
impl HistoryProjection {
    pub fn apply(&mut self, event: &Event);
    pub fn history(&self) -> &[HistoryItem];
    pub fn fold<'a>(events: impl IntoIterator<Item = &'a Event>) -> Self;
}
```

- `apply` 的映射规则见 conversation-history spec。`TurnStarted` 打开一个回合；`TurnComplete` 关闭回合；`TurnAborted`、`Error`（带 `turn_id`）以及遇到下一个 `TurnStarted` 时，先对未关闭的回合执行修复（`fill_missing_tool_results`），再关闭。`fold` 结束时，如果仍有未关闭的回合，同样执行修复。
- 修复只在投影中完成，不另外发出补偿事件，所以无论是正常出错，还是进程崩溃后日志被截断，都只走同一条修复路径。adapter 看到的事件就是 `Error`，以及没有 `ToolCallBegin` 的那些调用。
- 恢复路径中，由日志 fold 出的 history 和线程运行时增量维护的 history 走同一份 `apply` 代码。测试中断言二者相等。

### D5：`TurnMachine` 改为只读 history

`TurnMachine` 不再持有 `&mut Vec<HistoryItem>`，只负责状态转移，并通过返回值告诉驱动器需要记录什么：`on_model_output` 返回过滤后的 `(content, tool_calls)` 和 `ModelOutputEffect`。`AgentLoop` 持有一个 `Recorder`（存储 + 投影 + thread_id + 提交 ID + 回合 ID + clock），所有事件都通过 `recorder.record(msg)` 写入。构造模型请求时读取 `recorder.history()`，并在副本末尾追加 reminder。

- 备选方案是保留 `TurnMachine` 直接 push，事件只作为旁路记录。这会产生两个事实来源，history 与日志可能不一致，与目标冲突，因此不采用。

### D6：线程构造与恢复

```rust
TapestryThread::builder(model, tools)
    .clock(clock)             // 可选
    .event_store(store)       // 可选，默认 InMemoryEventStore
    .spawn()                  // 新线程
// 或
    .resume(thread_id).await? // 读取日志并 fold，返回已有线程
```

原有的 `spawn`/`spawn_with_clock` 保留为 builder 的简写。恢复时，`Session.configured = false`。

### D7：`ThreadHandle` 的消费接口

- 删除 `next_event`。新增 `subscribe(after_log_position) -> EventSubscription`，通常用 `subscribe(0)` 获取全部事件。
- `collect_events_for` 移到 `EventSubscription` 上，语义不变（读到匹配提交的终止事件为止）。因为每个订阅者有独立的游标，它不再吞掉其他消费者的事件。
- 订阅可以在提交之前或之后建立；从 0 订阅时，提交后建立也不会错过事件，测试中原有的竞态问题随之消失。

### D8：协议模块调整

- `EnvironmentContext`、`SystemReminder` 移入 `common/mod.rs`，或单独放在 `common/context.rs` 中并重导出。
- `HistoryItem` 移入 `model.rs`。删除 `history.rs`。
- 事件中的新载荷：`UserMessageEvent { name, content }`、`AssistantMessageEvent { content, tool_calls }`、`EnvironmentContextUpdatedEvent { context }`、`SystemReminderEvent { iteration, reminder }`。

## Risks / Trade-offs

- [追加失败时无法再追加 `Error` 事件] → worker 通过 `tracing::warn!` 记录日志，并结束当前提交；订阅者只会看到日志在某个回合中途停止。投影的截断修复规则保证 history 仍然合法。
- [内存后端无界增长] → 与现在 `Vec<HistoryItem>` 的无界增长性质相同。保留和裁剪策略留给持久化后端的 change。
- [`collect_events_for` 等待第二次 `UpdateSettings` 会挂起（(a) 遗留问题）] → 文档中说明；本 change 不改变该行为。
- [事件线格式整体变化] → 目前没有外部消费者；为每个新事件和信封补充往返测试。
- [每次写事件都要等待存储] → 内存后端的开销可以忽略。对于远程后端，可以以后在 trait 层增加批量追加；trait 中暂不预留。

## Migration Plan

单次源码级迁移。如果实现中需要分步提交，顺序为：先做 protocol 类型（信封、事件、模块调整），再做 core 的存储和投影，然后接入 worker 和 `AgentLoop`，最后更新文档和示例。回滚方式为 revert。
