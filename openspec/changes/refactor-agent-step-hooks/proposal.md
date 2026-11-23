# Proposal

## Why

当前 Agent 已提供 `reasoning`、`acting`、`forward` 及带钩子的版本，但生产路径绕过原始方法，覆盖这些方法不能可靠定制实际执行；同步策略回调又与执行循环重复维护状态。需要一套默认运行标准 ReAct、允许中间件根据步骤结果调整下一步的统一调度契约。

## What Changes

- 默认 `forward` 调度 `TurnStep::CallModel`、`TurnStep::CallTool`、`TurnStep::Done`；`CallTool` 表示完整工具批次，不支持批次内提前停止，也不提供自定义步骤扩展。
- **BREAKING** 删除 `Agent::next_step`、`on_model_output`、`on_tool_results`，默认调度逻辑归入 `forward`，移除 `ReactAgent` 中重复的策略状态机。
- **BREAKING** 调整步骤签名：`reasoning` 接收已构建的模型请求；单工具 `acting` 是可覆盖执行点，`acting_with_hooks` 负责完整批次的逐工具包装；`forward` 返回回合结果，完成事件由 `forward_with_hooks` 统一记录。
- `reasoning_with_hooks`、`acting_with_hooks`、`forward_with_hooks` 必须调用对应原始方法，保证下游覆盖生效。模型提供方 IO 仍由 `ModelClient::stream` 实现，消费流及运行时事件记录保留在 core 的 Agent 执行层，不让 model 依赖 `Recorder`。
- 在 `Middleware` 增加默认无操作的 `after_step`，接收类型化步骤结果和可修改的默认下一步；`before_turn` 提供初始步骤调整入口。
- **BREAKING** 全部钩子统一按中间件注册顺序执行，包括所有 `after_*`。`Done` 为不可撤销的终止决定，后续无错误的中间件仍执行，但不得重启流程。
- 根据整批工具结果选择 `Done` 时正常结束，不再请求模型；保留事件顺序、结果配对、用量累计、工具迭代限制和模型调用硬上限。

## Capabilities

### New Capabilities

- `agent-turn-policy`: 定义 Agent 步骤覆盖、封闭步骤调度、默认 ReAct 和运行时不变量的契约。该能力已在未归档的 `merge-turn-machine-into-agent` 中提出但尚未进入主规格；本变更重定其后续目标，不另建近义能力。
- `agent-middleware`: 定义中间件绑定、统一注册顺序、步骤后调度覆盖、不可撤销终止和错误传播。该能力已在未归档的 `add-agent-middleware` 中提出但尚未进入主规格；本变更明确取代其中后置钩子逆序的约定。

### Modified Capabilities

- `react-turn-loop`: 将无工具结束、工具后继续以及工具失败后继续的规则明确为默认调度，允许调度 hook 正常提前结束或选择其他合法下一步。

## Impact

- 涉及 `tapestry-core/src/agent/`、`middleware/`、公开 re-export、线程装配与自定义 Agent 测试，以及 `docs/architecture.md`。
- 下游 Agent 实现及依赖后置钩子逆序的中间件需要迁移；保留按回合创建 Agent 和跨回合共享绑定的模型、工具、中间件。
- 不新增依赖，不修改 `tapestry-protocol` 线类型、结束原因枚举或外部 adapter 映射。
- 不实现流式内容缓冲/审查、并行工具、批次中断、自定义工作流步骤、错误路径的 finally 钩子。
- 既有工作区修改不在本提案生成阶段改动；实施以当前工作区为起点。归档时须协调上述未归档变更，避免旧规格覆盖本变更新契约。
