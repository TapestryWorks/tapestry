# Design

## Context

当前实现分成三层：

```
thread worker ──▶ Arc<dyn Agent>::forward(&mut Recorder, ...)   async，带 IO，唯一实现是 AgentLoop
                     └─ AgentLoop：stream_model / run_tool / middleware / record
                          └─ TurnMachine：next_step / on_model_output / on_tool_results，纯同步
```

现有约束：

- `thread-events` 要求 `ModelRequestCompleted(i)` 包住第 i 次迭代的助手消息和工具事件，所以一次迭代必须按“模型 → 工具 → Completed”作为整体执行。现在的 `TurnStep::RunTools` 不能作为独立步骤由主循环驱动，这就是 `agent_loop.rs` 中出现死分支的原因。
- `add-agent-middleware` 的钩子位置和改写语义属于既有契约，本变更不改。
- 动机见 proposal.md（Why），可观察行为见 `specs/agent-turn-policy/spec.md`。

## Goals / Non-Goals

**Goals:**

- `Agent` 改为唯一的回合策略抽象，直接替代 `TurnMachine`。它是纯同步的，看不到 IO 和 `Recorder`。
- `AgentLoop` 改为唯一的驱动器，负责全部事件记录、迭代编号、工具配对和不变量校验。
- 默认策略下，现有 `agent_loop.rs` 和 `thread.rs` 的行为测试不改断言即可通过。

**Non-Goals:**

- 策略访问事件快照或 history。以后如有需要，可以在 `on_model_output` 的输入中增加只读快照，这不影响本设计。
- 策略改写模型请求或响应内容，这类需求已由 middleware 承担。

## Decisions

### D1：`Agent` 是按回合创建的有状态策略，由工厂负责创建

```rust
// agent/agent_trait.rs
pub enum TurnStep {
    CallModel { allow_tools: bool, reminder: Option<SystemReminder> },
    Done { finish: FinishReason, last_agent_message: Option<String> },
}

pub trait Agent: Send {
    fn next_step(&self) -> TurnStep;
    /// 返回需要执行的工具调用，必须是 `tool_calls` 的保序子序列。
    fn on_model_output(&mut self, content: Option<&str>, tool_calls: &[ToolCall])
        -> Result<Vec<ToolCall>, CoreError>;
    /// 只在上一次输出保留了调用时调用，`results` 与保留的调用一一对应。
    fn on_tool_results(&mut self, results: &[ToolResult]) -> Result<(), CoreError>;
}

pub type AgentFactory = Arc<dyn Fn(&ThreadConfig) -> Box<dyn Agent> + Send + Sync>;
```

- 理由：`TurnMachine` 本来就是每个回合 `new` 一次、用 `&mut self` 推进状态。如果把状态显式放进 trait，最接近“合并”的形式就是让 `Agent` 本身作为每个回合的状态对象，跨回合共享的只有工厂闭包。只要求 `Send`，不要求 `Sync`，因为它只在单个回合的 future 内使用。
- 备选一：`Agent` 使用关联类型 `type Turn` 加 `begin_turn`。这样做无法作为 `dyn` 使用，线程侧必须泛型化，因此不采用。
- 备选二：`Agent` 本身是工厂，再另外定义 `AgentTurn` trait。这样还是两个 trait，没有达到合并目的，因此不采用。

### D2：驱动器持有迭代编号，`TurnStep` 不再携带 `iteration`

`ModelRequestStarted`、`SystemReminder` 和 `ModelRequestCompleted` 中的 `iteration` 由 `AgentLoop` 计数，从 0 开始连续递增。策略如果需要迭代数，可以自行计数。

- 理由：迭代编号属于事件契约，不应由可替换代码提供。这也避免了策略给出跳号或重复编号。

### D3：一次迭代由驱动器原子执行，删除 `RunTools`

```
loop {
  match agent.next_step() {
    Done{..} => break,
    CallModel{allow_tools, reminder} => {
      if iteration > max_tool_iterations { Err(Internal) }          // 硬上限
      ModelRequestStarted / SystemReminder?
      request → before_model* → stream_model → after_model*
      kept = agent.on_model_output(content, &response.tool_calls)?
      校验 kept 是 response.tool_calls 的保序子序列，否则 Err(Internal)
      AssistantMessageStart? / AssistantMessage{content, kept}（二者都空时不发）
      dropped > 0 → Warning
      if !kept.is_empty() { results = run_tool* ; agent.on_tool_results(&results)? }
      ModelRequestCompleted
      iteration += 1
    }
  }
}
```

- 理由：保持 `ModelRequestCompleted` 包住工具事件的既有顺序。工具批次作为 `on_model_output` 的返回值，并行执行的扩展点仍在驱动器内部。
- 模型调用前一律提供工具会让模型白白返回调用。是否提供工具仍由 `allow_tools` 控制；`ignored_tool_calls` 统一表示为“丢弃数量 = 模型返回数 - 保留数”，由驱动器负责发警告。`ModelOutputEffect` 因此删除。

### D4：保序子序列的校验方式

逐个扫描模型返回的调用：保留列表中的每一项都必须按顺序与某个模型调用完全相等（`ToolCall: PartialEq`，id、name、arguments 都相同）。如果本次 `allow_tools == false`，保留列表必须为空。校验失败时返回 `CoreError::Internal`，此时还没有记录 `AssistantMessage`，history 中不会出现悬空调用。

- 备选：只比较 id。这样策略可以悄悄改写参数，而改写参数已经由 middleware 的 `before_tool` 承担，并且有恢复 id 和 name 的保护，因此不采用。

### D5：硬上限复用 `max_tool_iterations + 1`

默认 `ReactAgent` 最多发起 `max_tool_iterations + 1` 次模型调用（最后一次是强制收尾），所以这个上限对默认行为没有影响。自定义策略超出上限时返回 `Internal` 错误，而不是驱动器替策略收尾，因为驱动器不知道正确的结束原因。

- 备选：新增独立配置 `max_model_calls`。这会增加 `ThreadSettings` 字段并改动协议，目前没有需求，因此不采用。

### D6：`ReactAgent` 是 `TurnMachine` 的直接迁移

把 `turn.rs` 迁入 `agent/react_agent.rs`，`Phase` 改为 `NeedModel / AwaitTools / Finished`：

- `ReactAgent::new(max_tool_iterations)` 内部维护迭代计数。到达上限时，`next_step` 返回 `CallModel { allow_tools: false, reminder: Some(ToolLimitReached) }`。
- 在 `allow_tools == false` 的迭代中，`on_model_output` 返回空的保留列表，警告由驱动器发出；否则原样保留全部调用。
- `on_tool_results` 只做状态转移，配对由驱动器保证，原来的 ID 校验移到驱动器的保序子序列校验中。
- 原 `turn.rs` 中的纯同步单测随之迁移。删除 `turn.rs`，`TurnStep` 移入 `agent_trait.rs` 并去掉 serde，`lib.rs` 继续在根部导出 `TurnStep`。

### D7：`AgentLoop` 和线程装配方式

- `AgentLoop` 新增字段 `agent_factory: AgentFactory`，默认值为 `|c| Box::new(ReactAgent::new(c.max_tool_iterations))`；新增 `with_agent_factory(f)`。`new`、`with_clock` 和中间件相关方法不变。
- `AgentLoop::forward` 保持签名，变成固有方法，不再实现 `Agent`。
- `ThreadBuilder::agent(Arc<dyn Agent>)` 替换为 `ThreadBuilder::agent_loop(AgentLoop)`，线程侧持有 `Arc<AgentLoop>`，`process_submission` 接收 `&AgentLoop`。
- 备选：保留一个异步 `Agent::forward` trait 作为“整回合”逃生口。这会重新引入可以绕过事件契约的入口，与本变更的目标冲突，因此不采用。

## Risks / Trade-offs

- [`Agent` 名称语义改变，下游 `impl Agent` 会编译失败] → 仓库内没有其他实现；在 proposal 中标记为 **BREAKING**，并在 `docs/architecture.md` 中写明迁移方式：整回合逻辑改写为策略，或通过 middleware 实现。
- [失去非 ReAct 形态的扩展能力] → 这是选择方案 B 时有意接受的代价。以后确有需要时，可以在驱动器之上新增独立入口，而不是恢复 `Agent::forward`。
- [硬上限复用 `max_tool_iterations` 会限制“强制再跑一轮”类策略] → 上限可以通过设置调大；需要时再拆出独立配置。
- [策略在 `on_*` 中返回错误时会中断回合] → 与模型错误路径一致，由线程边界记录 `Error`，history 投影负责修复悬空调用。

## Migration Plan

单次提交完成：迁移 `ReactAgent`、改写驱动器、更新线程装配、更新导出与文档。回滚时直接回退该提交，不涉及数据迁移，事件格式不变。
