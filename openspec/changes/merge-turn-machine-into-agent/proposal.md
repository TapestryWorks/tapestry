# Proposal

## Why

现在有两个单实现的抽象，二者的职责划分不合理：

- `Agent` trait 是线程注入的回合级扩展点。它的 `forward` 会直接拿到 `&mut Recorder`，所以自定义实现可以随意记录事件，绕开 `AssistantMessageStart`/`Delta`/`AssistantMessage`、`ToolCallBegin`/`ToolCallEnd` 的配对契约。`add-agent-middleware` 已经为中间件专门关闭了这个口子，但 `Agent` 仍然开着。
- `TurnMachine` 是 `AgentLoop` 内部固定使用的同步决策器，回合策略（何时继续、何时结束、丢弃哪些调用）无法替换。它的状态机也只用了一半：因为 `ModelRequestCompleted` 必须包住同一迭代的工具事件，驱动器在模型输出后直接内联执行工具，`TurnStep::RunTools` 分支永远走不到。

把 `TurnMachine` 合并进 `Agent` 后，`Agent` 只负责纯同步的回合决策，`AgentLoop` 成为唯一的 IO 驱动器，负责所有事件记录和不变量校验。这样扩展点可以替换回合策略，但无法破坏事件契约。

## What Changes

- **BREAKING** `Agent` trait 改为同步的回合策略，用于替代 `TurnMachine`：每个回合有一个实例，由驱动器询问下一步并回灌模型输出和工具结果，不接触 `Recorder`、模型、工具或任何 IO。
- **BREAKING** 删除 `TurnMachine`、`ModelOutputEffect` 和 `TurnStep::RunTools`。现有 ReAct 逻辑迁入默认实现 `ReactAgent`，行为保持不变。
- **BREAKING** `AgentLoop` 不再实现 `Agent`，改为具体的驱动器：持有模型、工具、时钟、中间件和一个按回合创建 `Agent` 的工厂（默认创建 `ReactAgent`）。
- 驱动器负责校验策略输出，确保自定义策略无法破坏契约：策略只能按原顺序丢弃模型返回的工具调用，不能新增或改写；工具结果由驱动器按调用配对；单回合的模型调用次数有硬上限；被丢弃的调用统一由驱动器发出 `Warning`。
- **BREAKING** `ThreadBuilder::agent(Arc<dyn Agent>)` 改为注入 `AgentLoop`（`ThreadBuilder::agent_loop`）；自定义策略通过 `AgentLoop::with_agent_factory` 设置。
- `TurnStep` 去掉 serde 派生（它是 core 内部类型，没有序列化消费者）。
- 事件顺序、ID 职责和终止语义保持不变；使用默认策略时现有行为完全一致。
- 非目标：非 ReAct 形态的 agent（如 planner、多 agent 路由）、异步或带 IO 的策略、并行工具、策略直接记录事件、策略状态持久化。

## Capabilities

### New Capabilities

- `agent-turn-policy`：可替换的回合策略与驱动器之间的契约，包括策略可以决定的内容（继续或结束、是否提供工具、提醒、结束原因、丢弃哪些工具调用），以及驱动器强制的不变量（调用只能按序丢弃、结果配对、调用次数硬上限、警告由驱动器发出、事件契约不可绕过）。

### Modified Capabilities

（无。默认的 `ReactAgent` 保持 `react-turn-loop`、`thread-events`、`tool-execution` 的现有行为，`agent-middleware` 的钩子位置也不变。）

## Impact

- 代码：`crates/tapestry-core/src/agent/agent_trait.rs`（新的 `Agent` trait）、新增 `agent/react_agent.rs`（由 `turn.rs` 迁入）、删除或精简 `turn.rs`（只保留 `TurnStep`）、`agent/agent_loop.rs`（改为驱动策略并负责校验）、`thread.rs`（builder 注入 `AgentLoop`）、`lib.rs`（re-export 调整与兼容测试）。
- 公开 API：**BREAKING**。`Agent` 语义改变；`TurnMachine`、`ModelOutputEffect` 被删除；`ThreadBuilder::agent` 被替换。仓库内只有 `thread.rs` 和 `lib.rs` 的测试使用这些接口。
- 协议层与 adapter：无改动。
- 文档：更新 `docs/architecture.md`（Codex 映射表、turn/agent 章节和时序图）和 `.github/copilot-instructions.md` 中关于 `AgentLoop` 与 `Agent` 的描述。
- 前置：基于已完成的 `add-agent-middleware`，中间件钩子位置不变。
