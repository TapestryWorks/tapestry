# Design

## Context

`AgentLoop::forward`（`crates/tapestry-core/src/agent/agent_loop.rs`）负责驱动纯同步的 `TurnMachine`，执行模型调用（`stream_model`）和工具调用（`run_tool`），并通过 `Recorder` 记录每个事件。事件日志是唯一事实来源，history 由日志投影得到。事件顺序、`ToolCallBegin`/`ToolCallEnd` 配对、`ToolCallId` 与 `MessageId` 的职责都属于可观察契约。动机见 proposal.md。

## Goals / Non-Goals

**Goals:**
- 在 `AgentLoop` 内部提供六个前置/后置钩子，节点位置和改写语义见 specs/agent-middleware/spec.md。
- 中间件无法破坏事件顺序和 ID 契约。
- 不注册中间件时零行为变化，现有构造函数和测试不变。

**Non-Goals:**
- 洋葱式 `next` 包装（重试、超时、包装流）。
- 中间件直接记录事件，或新增 `EventMsg` 变体。
- 中间件状态的持久化与 resume 恢复。
- `TurnMachine` 感知中间件，例如“吞掉完成以强制再跑一轮”。

## Decisions

### 1. 前置/后置钩子，而不是洋葱模型

trait 位于 `crates/tapestry-core/src/middleware/middleware_trait.rs`（沿用 `agent/agent_trait.rs` 的布局），由 `middleware/mod.rs` 导出，并在 `lib.rs` 重新导出：

```rust
#[async_trait]
pub trait Middleware: Send + Sync {
    async fn before_turn(&self, ctx: &HookCtx<'_>) -> Result<(), CoreError> { Ok(()) }
    async fn after_turn(&self, ctx: &HookCtx<'_>) -> Result<(), CoreError> { Ok(()) }
    async fn before_model(&self, ctx: &HookCtx<'_>, request: &mut ModelRequest) -> Result<(), CoreError> { Ok(()) }
    async fn after_model(&self, ctx: &HookCtx<'_>, response: &mut ModelResponse) -> Result<(), CoreError> { Ok(()) }
    async fn before_tool(&self, ctx: &HookCtx<'_>, call: &mut ToolCall) -> Result<ToolDecision, CoreError> { Ok(ToolDecision::Continue) }
    async fn after_tool(&self, ctx: &HookCtx<'_>, call: &ToolCall, result: &mut ToolResult) -> Result<(), CoreError> { Ok(()) }
}

pub enum ToolDecision { Continue, Skip(ToolResult) }
```

- 理由：洋葱模型的 `next` 需要把 `&mut Recorder` 穿过多层异步闭包，借用关系复杂，而且中间件可以绕过或重复执行核心步骤，难以守住事件契约。前置/后置钩子满足“在关键节点前后执行”的目标，事件记录权始终在 `AgentLoop` 手里。
- 备选：外部装饰 `Agent`/`ModelClient`/`Tool`。不采用，原因见 proposal.md。

### 2. `HookCtx` 只读，不暴露 `Recorder`

```rust
pub struct HookCtx<'a> {
    pub thread_id: ThreadId,
    pub turn_id: TurnId,
    pub iteration: Option<u32>,   // 模型/工具钩子为 Some，回合钩子为 None
    pub config: &'a ThreadConfig,
}
```

- 理由：中间件只能通过改写参数和返回值来影响流程，所有事件仍由 `AgentLoop` 按既有顺序记录。
- 备选：传 `&mut Recorder`。不采用，因为可能打乱 Start/Delta/Message 和 Begin/End 的配对。

### 3. 钩子在流程中的位置

```
forward
 TurnStarted
 before_turn*            (A→B)
 EnvironmentContextUpdated? / UserMessage
 loop CallModel
   ModelRequestStarted / SystemReminder?
   request = 组装(events 快照)
   before_model*         (A→B)  &mut request
   stream_model           ← Delta 照常记录
   after_model*          (B→A)  &mut response
   content = response.content.or(streamed)   ← 最终内容为准
   machine.on_model_output → AssistantMessage
   for call:
     before_tool*        (A→B)  &mut call；遇到 Skip 时停止后续 before_tool
     恢复 call.id / call.name 为原值
     ToolCallBegin(改写后的参数)
     execute | not_found | Skip 给出的结果
     after_tool*         (B→A)  &mut result
     回写 result.call_id / tool_name
     ToolCallEnd
   ModelRequestCompleted(usage = 改写后的 response.usage)
 after_turn*             (B→A)
 TokenCount / TurnComplete
```

- `before_turn` 放在 `TurnStarted` 之后：回合已经开始，钩子报错时日志里有 `TurnStarted` 和随后的 `Error`，与现有错误路径一致。
- `before_tool` 放在 `ToolCallBegin` 之前，这样记录的参数就是实际执行的参数，并且钩子报错时不会留下未配对的 Begin。
- `after_tool` 放在 `ToolCallEnd` 之前。此时报错会留下未配对的 Begin，由现有的投影修复机制补上 `failed` 结果，与其他错误路径一致。
- 调用 ID 和工具名的保护：`before_tool` 结束后恢复原值，`after_tool` 结束后沿用现有做法，强制回写结果的 `call_id` 和 `tool_name`。这样 `TurnMachine::on_tool_results` 的校验不受影响。

### 4. 构造方式：链式追加，默认为空

```rust
AgentLoop::with_clock(model, tools, clock)
    .with_middleware(Arc::new(A))
    .with_middlewares([b, c]);
```

- `AgentLoop` 新增 `middlewares: Vec<Arc<dyn Middleware>>` 字段，`new`/`with_clock` 中初始化为空。
- `before_*` 正序遍历，`after_*` 逆序遍历。列表为空时只是一次空遍历。
- 线程侧不新增 `ThreadBuilder::middleware`，使用已有的 `.agent(...)` 注入，避免与自定义 agent 的语义冲突。

### 5. 状态归中间件自身所有

中间件以 `&self` 被调用，并且必须满足 `Send + Sync`，因此状态通过内部可变性保存，例如 `Mutex`、`AtomicU64`。核心不提供状态容器，也不写日志。状态作用域等于实例作用域：每个线程各建一个实例，状态按线程隔离；多个线程共享同一个 `Arc` 实例，状态就共享。

### 6. 错误传播

钩子返回 `CoreError`，`AgentLoop` 直接用 `?` 传播。现有的线程边界会把它转换为与提交关联的 `EventMsg::Error`。中间件自身不构造错误事件。

## Risks / Trade-offs

- [`after_model` 改写文本后，与已记录的 Delta 不一致] → 沿用“最终内容为准”的既有规则，history 只取 `AssistantMessage`。在文档中说明这一点。
- [`after_model` 新增的工具调用缺少提供方 ID] → 中间件需要自行用 `ToolCallId::generate()` 生成 ID，在 trait 文档中说明。
- [中间件状态在 resume 后丢失] → 明确列为非目标，以后可以新增 `snapshot`/`restore`。
- [`after_tool` 报错会留下未配对的 `ToolCallBegin`] → 与现有工具路径中途出错的处理一致，依赖投影修复。
- [钩子是 async，可能阻塞回合] → 回合本来就是串行执行，这是可接受的代价，由中间件作者负责。

## Migration Plan

只新增 API，无需迁移。回滚时移除 `middleware` 模块和 `AgentLoop` 中的钩子调用即可。
