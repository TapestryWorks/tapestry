# Proposal

## Why

`AgentLoop` 目前把“模型 → 工具 → 模型”的流程写死在内部，调用方无法在关键节点（回合开始/结束、模型调用前后、工具调用前后）插入自定义处理，例如日志、参数校验、结果改写或拦截某次工具调用。外部包装 `Agent`、`ModelClient` 或 `Tool` 都不够用：包装 `Agent` 时后置逻辑只能在 `TurnComplete` 之后运行，违反终止事件约定；包装 `ModelClient`/`Tool` 拿不到回合上下文，也覆盖不到 `ToolCallBegin`/`ToolCallEnd` 和工具未找到的路径。参考 AgentScope 的 middleware 思路，需要在核心流程内部提供一等公民的中间件扩展点，同时不破坏现有事件顺序和 ID 契约。

## What Changes

- 新增 `tapestry-core` 模块 `middleware`，在 `crates/tapestry-core/src/middleware/middleware_trait.rs` 定义 `Middleware` trait（异步、`Send + Sync`，以 `Arc<dyn Middleware>` 保存），所有钩子都有空操作默认实现，中间件只需实现关心的节点：
  - `before_turn` / `after_turn`：回合开始后、回合结束前（`TokenCount`/`TurnComplete` 之前）。
  - `before_model` / `after_model`：每次模型调用前可改写 `ModelRequest`，拿到最终响应后可改写 `ModelResponse`。
  - `before_tool` / `after_tool`：每次执行模型推理出的工具调用前，可改写参数或用给定结果跳过执行；拿到工具结果后可改写结果。
- 钩子接收只读的 `HookCtx`（线程、回合、迭代序号和线程配置），不接触 `Recorder`。事件仍由 `AgentLoop` 统一记录，现有事件顺序和 ID 职责保持不变。
- 中间件可以在自身结构内保存自定义状态（内部可变性），状态的生命周期与中间件实例相同，不写入事件日志。
- `AgentLoop` 支持在创建时集成零个或多个中间件：`new`/`with_clock` 保持不变且默认没有中间件，新增链式的 `with_middleware`、`with_middlewares`。`before_*` 按注册顺序执行，`after_*` 按逆序执行。
- 线程侧不新增入口：需要中间件时构造带中间件的 `AgentLoop`，再通过已有的 `ThreadBuilder::agent` 注入。
- 首批使用 `MockModelClient` + `EchoTool`（`tool:echo` 路径）验证。
- 非目标：洋葱式 `next` 包装（重试、超时）、中间件提供工具、权限审批事件、上下文压缩、system prompt 变换管线、中间件状态持久化与 resume 恢复、新增事件类型。

## Capabilities

### New Capabilities

- `agent-middleware`：在 agent 回合的关键节点按注册顺序执行中间件的可观察行为，包括节点位置、可改写的数据、工具跳过语义、多中间件顺序、错误处理，以及无中间件时行为不变。

### Modified Capabilities

（无。没有中间件时现有 `react-turn-loop`、`tool-execution`、`thread-events` 的行为不变。）

## Impact

- 代码：新增 `crates/tapestry-core/src/middleware/`（`mod.rs`、`middleware_trait.rs`）；修改 `agent/agent_loop.rs`，在各节点调用中间件；`lib.rs` 重新导出 `Middleware`、`HookCtx`、`ToolDecision`。
- 公开 API：只有新增，不破坏现有构造函数和 `Agent` trait。
- 协议层与 adapter：无改动，不新增事件。
- 文档：更新 `docs/architecture.md`，说明中间件节点与事件的相对位置。
- 依赖：无新增依赖。
