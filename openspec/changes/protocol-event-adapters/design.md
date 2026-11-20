# Design

## Context

前置条件：`event-sourced-thread-history` 和 `streaming-assistant-output` 已完成。事件信封包含 `log_position`、`thread_id`、`turn_id`、`timestamp_ms`；助手消息有 Start、Delta、`AssistantMessage` 三种事件，并共享 `message_id`；消费方通过 `EventSubscription` 按序读取单个线程的事件。

外部协议版本：
- **AG-UI 1.0**（docs.ag-ui.com/spec/1.0/schema）：`type` 取值为 SCREAMING_SNAKE，字段为 camelCase，`timestamp` 是 Unix 毫秒整数。
- **A2A 1.0**（a2aproject/A2A `specification/a2a.proto`，`lf.a2a.v1`）：`StreamResponse` 包含 `task`、`message`、`statusUpdate`、`artifactUpdate` 四种之一；`TaskState` 在 JSON 中为 `TASK_STATE_*`；`TaskStatusUpdateEvent` 没有 `final` 字段；`TaskArtifactUpdateEvent` 带有 `append` 和 `lastChunk`；`Part` 是 oneof，文本形式为 `{"text": ...}`。

## Goals / Non-Goals

**Goals:**
- protocol 中的契约足够小，任何协议都能以“状态机 + 逐条映射”的方式实现。
- 两个参考实现完全确定，可以通过纯单元测试验证，不需要 IO 和运行时。

**Non-Goals:**
- 不做入站转换（AG-UI `RunAgentInput`、A2A `SendMessage` → `Command`）。
- 不做传输层（SSE、JSON-RPC 信封、HTTP 服务）、不做 Agent Card，也不提供 MESSAGES_SNAPSHOT/STATE_* 等快照类事件。
- 不做跨线程聚合：一个转换器实例只处理一个线程。

## Decisions

### D1：契约放在 protocol 的 `adapter` 模块

```rust
#[serde(rename_all = "snake_case")]
pub enum EventCategory { Session, Lifecycle, Message, Tool, Model, Context, Usage, Diagnostic }

impl EventMsg { pub fn category(&self) -> EventCategory }   // 穷尽 match，新增事件时编译期强制归类

pub struct EventFilter { /* 位集合 */ }
impl EventFilter {
    pub fn all() -> Self; pub fn none() -> Self;
    pub fn with(self, c: EventCategory) -> Self;
    pub fn matches(&self, event: &Event) -> bool;
}

pub trait EventAdapter {
    type Output;
    fn adapt(&mut self, event: &Event) -> Vec<Self::Output>;
    fn finish(&mut self) -> Vec<Self::Output> { Vec::new() }
}
```

- 选择同步、`&mut self`、返回 `Vec` 的形式：转换本身是纯计算，不应该绑定到 async 运行时；需要可变状态，是因为 A2A 要跟踪 artifact 是否为首块、task 是否已进入终态，AG-UI 要跟踪文本消息是否已打开。
- 备选方案是 `Iterator` 或 `Stream` 适配器。它们会把调用方绑定到特定的组合方式，`Vec` 更简单，也方便测试。
- 每个事件只属于一个类别，并且是 `EventMsg` 上的固有属性，不由各 adapter 自行定义，这样过滤语义在所有消费方之间一致。
- 这样做符合“protocol 只放契约”的约定：trait 是契约，`category()` 是对数据的分类，没有 IO 或策略。

### D2：每种协议一个 crate，线类型自行定义

`crates/tapestry-adapter-agui` 和 `crates/tapestry-adapter-a2a` 只依赖 `tapestry-protocol`、`serde` 和 `serde_json`。

- 不使用 `a2a-rs`：它是包含传输、存储和 LLM 的完整框架，依赖很重。AG-UI 目前没有成熟的官方 Rust SDK。我们只需要出站事件的那一小部分类型，自行定义的成本低，版本也完全可控。
- 每个 crate 的根文档中标明对应的规范版本。线格式测试使用规范示例中的 JSON 结构，作为对照样本。

### D3：AG-UI 映射细节

`AguiAdapter { custom: EventFilter, open_text: Option<MessageId>, open_run: Option<TurnId> }`

| Tapestry | AG-UI |
|---|---|
| TurnStarted | RUN_STARTED{threadId, runId} |
| ModelRequestStarted/Completed | STEP_STARTED/STEP_FINISHED{stepName:`model_request:{iteration}`} |
| AssistantMessageStart | 不输出，只记录 |
| 第一个 AssistantMessageDelta | TEXT_MESSAGE_START{messageId, role:"assistant"} + CONTENT |
| 后续 Delta | TEXT_MESSAGE_CONTENT{messageId, delta} |
| AssistantMessage | 有文本：如果尚未打开文本消息，补发 START 和整段 CONTENT，然后 END；接着对每个工具调用输出 TOOL_CALL_START/ARGS/END |
| ToolCallEnd | TOOL_CALL_RESULT{messageId:`{tool_call_id}:result`, toolCallId, content} |
| TurnComplete | RUN_FINISHED |
| TurnAborted / Error | 先关闭已打开的文本消息，再输出 RUN_ERROR{message, code} |
| 其他事件 | 被 `custom` 过滤器匹配时输出 CUSTOM{name:`tapestry.<type>`, value}，否则不输出 |

- TOOL_CALL_RESULT 的 `messageId` 使用由工具调用 ID 派生的确定性值，这样不需要引入 uuid，测试也可以复现。
- AG-UI 要求每条 CONTENT 的 `delta` 非空，因此跳过空的增量。

### D4：A2A 映射细节

`A2aAdapter { artifacts_started: HashSet<MessageId>, terminal: HashSet<TurnId> }`

| Tapestry | A2A `StreamResponse` |
|---|---|
| TurnStarted | `task{id:turn_id, contextId:thread_id, status:{state:WORKING}}` |
| AssistantMessageDelta | `artifactUpdate{artifact{artifactId:message_id, parts:[{text}]}, append: 是否为非首块, lastChunk:false}` |
| AssistantMessage(有文本) | `artifactUpdate{..., parts:[{text:完整文本}], append:false, lastChunk:true}` |
| TurnComplete | `statusUpdate{status:{state:COMPLETED}}` |
| TurnAborted | `statusUpdate{status:{state:CANCELED}}` |
| Error(有 turn_id) | `statusUpdate{status:{state:FAILED, message{role:AGENT, parts:[{text}]}}}` |

- 最终消息以 `append:false` 覆盖之前通过增量拼出的内容，这与 (c) 中“以最终文本为准”的规则一致。
- 状态消息的 `messageId` 由回合 ID 派生（`{turn_id}:status`）。`TaskStatus.timestamp` 是可选的 ISO 8601 字符串，本次不填，以免引入 chrono。
- task ID 取自信封中的 `turn_id`；没有 `turn_id` 的事件一律不输出。

## Risks / Trade-offs

- [自行定义的线类型与官方规范有偏差] → 用规范中的示例 JSON 做序列化对照测试；文档中标明规范版本；规范升级时只需修改对应的 crate。
- [AG-UI 前端期望先收到 MESSAGES_SNAPSHOT 等快照] → 不在本次范围；以后可以基于 history 投影生成。
- [A2A 中，一个回合内可能产生多个 artifact（工具调用之前的文本也算）] → 这符合 A2A 的语义（一个 task 可以有多个 artifact）。如果以后只想暴露最终答案，可以给 adapter 增加配置项。
- [`StreamResponse` 是 oneof，JSON 表示依赖 proto3 JSON 映射规则] → 测试中断言每条输出 JSON 恰好有一个顶层键。

## Migration Plan

纯新增，不需要迁移。回滚方式为删除这两个新 crate 和 `adapter` 模块。
