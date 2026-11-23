# Design

## Context

动机见 `proposal.md`。当前工作区已经把模型、工具、中间件绑定到按回合创建的 Agent，并提供异步执行方法；但 `*_with_hooks` 绕过原始方法，内部 `react_loop(..., hooks: bool)` 维护两种执行路径，`ReactAgent` 还通过 `Phase` 和同步回调重复维护流程状态。

约束：

- `ModelRequestCompleted(i)` 包住第 i 次模型产生的助手消息及整个工具批次；不能在模型步骤返回时就无条件发送完成事件。
- 事件日志是唯一事实来源，模型请求来自有序事件快照，工具调用 ID 不解析且不能被中间件重新配对。
- 已有 `ModelClient::stream` 完成模型提供方 IO；消费增量、生成消息 ID、记录事件属于 core 的运行时行为。
- 工作区已有未提交的 Agent 重构，实施必须基于这些修改，而不是按较旧的 `merge-turn-machine-into-agent/design.md` 恢复同步纯策略结构。
- `agent-turn-policy`、`agent-middleware` 目前只存在于先前未归档变更中。本变更沿用其能力名，但以本设计的新契约为后续目标。

## Goals / Non-Goals

**Goals:**

- 一个默认循环、三个封闭步骤；覆盖原始执行方法在标准入口下实际生效。
- 将数据转换与调度分开：数据钩子处理请求/响应，`after_step` 接收最终步骤结果并修改预先计算的下一步。
- 调度可以正常提前结束，但不能留下悬空调用、重复执行批次、绕过预算或重复记录完成事件。
- 保持 `Box<dyn Agent>`、按回合工厂、共享 `AgentBindings` 及默认无操作中间件可用。

**Non-Goals:**

- 不新增 `AgentExt`、泛型工作流引擎、任意自定义步骤或独立可插拔调度器。
- 不支持工具批次内正常中断、不增加并行工具。
- 不改协议线格式，不增加新的结束原因。
- 不把后置模型钩子变为增量过滤器，也不把 `after_turn` 改为错误路径的 finally。

## Decisions

### D1：保留 TurnStep，但不再用同步回调推进隐藏状态

计划结构如下，具体派生 trait 以字段类型约束为准：

```rust
pub enum TurnStep {
    CallModel {
        allow_tools: bool,
        reminder: Option<SystemReminder>,
    },
    CallTool,
    Done {
        finish: FinishReason,
        last_agent_message: Option<String>,
    },
}

pub enum StepOutput<'a> {
    Model(&'a ModelResponse),
    Tools(&'a [ToolResult]),
}

pub struct TurnOutcome {
    pub finish: FinishReason,
    pub last_agent_message: Option<String>,
}
```

`CallTool` 表示消费上下文里唯一待执行的完整有序批次，而不是携带可任意伪造的调用列表。`TurnContext` 管理迭代、累计用量、最后助手文本、待执行批次、尚未闭合的模型迭代和初始计划；通过清晰的访问/消费方法支持自定义循环，不能把同一批次取出执行两次。

删除 `next_step`、`on_model_output`、`on_tool_results` 和 `ReactAgent::Phase`。`ReactAgent` 作为标准 Agent 使用默认 `forward`，不再独立维护第二份迭代状态；配置和预算以 `TurnContext` 为准。

理由：`TurnStep` 表达“接下来做什么”，不再要求用户实现完整状态机才能定制执行。相较保留 `on_*`，不会出现 hook 调整了计划、旧状态机仍期待另一个步骤的冲突。

### D2：原始执行点与钩子包装器真正组合

入口关系固定为：

```text
AgentLoop::forward
  记录 TurnStarted / 可选环境上下文 / UserMessage
  创建本回合 Agent 和 TurnContext
  agent.forward_with_hooks
    before_turn(ctx, &mut initial_step)*
    self.forward(ctx) -> TurnOutcome
      CallModel -> self.reasoning_with_hooks
        prepare_request
        before_model*
        self.reasoning(ctx, request)
        after_model*
      CallTool -> self.acting_with_hooks(ctx, batch)
        对每个调用：
          before_tool*
          ToolCallBegin
          self.acting(call)，或使用 Skip 的替代结果
          after_tool*
          修正结果配对 / ToolCallEnd
    after_turn*
    TokenCount / TurnComplete
```

- `reasoning(ctx, request: ModelRequest)` 默认调用 `ModelClient::stream` 并消费流，返回已有 `ReasoningOutput`。把现有 `call_model` 的流消费逻辑并入这里，删除重复公开覆盖点；请求构建由包装器在 `before_model` 前完成。
- `acting(call: &ToolCall)` 是单工具原始执行点，默认从绑定注册表查找工具，未知工具生成 `not_found`。用它替代重复的 `execute_tool` 覆盖点。
- `acting_with_hooks(ctx, calls: Vec<ToolCall>)` 是批次入口，逐调用包装并调用 `self.acting`。不采用“全批次 before → 批量 acting → 全批次 after”，以保持参数改写和事件配对的现有顺序。
- `forward(ctx)` 是唯一默认调度循环，返回 `TurnOutcome`，不记录回合完成事件；它内部使用带钩子的模型/工具步骤。
- `forward_with_hooks(ctx)` 真正调用 `self.forward`，在正常返回后执行 `after_turn` 和一次回合完成记录。

`forward` 不是“禁用全部中间件”的入口，而是“不含回合级包装的流程主体”。不再保留 `hooks: bool` 双路径。

理由：模型调用细节已经在 model 实现中，模型不该为运行时记录器增加依赖；原始工具覆盖点采用单调用粒度，才能被现有逐工具中间件正确包裹。自定义整回合流程仍须遵守记录和预算契约；trait 覆盖能力不是阻止恶意实现直接写事件的安全沙箱。

### D3：after_step 专门调整调度

在 `Middleware` 上增加默认无操作方法：

```rust
async fn after_step(
    &self,
    ctx: &HookCtx<'_>,
    output: &StepOutput<'_>,
    next_step: &mut TurnStep,
) -> Result<(), CoreError>;
```

`before_turn` 增加可修改初始计划的参数；默认初始计划是结合预算生成的 `CallModel`。初始选择 `Done` 时不调用模型，零用量正常结束；无待执行批次却选择 `CallTool` 是显式错误。

`after_step` 由默认 `forward` 在每个成功模型步骤/完整工具批次后调用，不放进原始 `reasoning` 或 `acting`，也不对 `Done` 再调用一次。这样包装器不必猜测循环终止策略。

- 模型节点：先运行 `after_model`，据修改后的响应和预算生成默认计划，再提供只读模型结果给 `after_step`。
- 工具节点：所有调用的 `after_tool`、ID 修正和 `ToolCallEnd` 完成后，默认生成下一次 `CallModel`，再提供完整有序结果给 `after_step`。
- `HookCtx.iteration` 在同一个模型及其工具批次中保持同一值。
- 非终止计划逐中间件传递，后执行者可以修改前一个非终止计划；整个节点结束后验证最终跳转的合法性。

理由：不在 `after_model` 和每个 `after_tool` 都加入不同的跳转协议；一个调度节点能稳定表达“根据整批结果提前结束”。工具失败值仍是数据，正常终止使用 `Done`，不借用 `CoreError` 或 `Interrupt`。

### D4：所有 hook 按注册顺序，Done 锁定终止

所有节点均使用注册顺序；移除后置钩子中的逆序遍历。已有 `before_tool` 返回 `Skip` 时短路余下工具前钩子的语义保持，所有工具后钩子仍按注册顺序执行；任一钩子出错仍立即停止。

调度节点进入时如果默认计划已为 `Done`，其终止信息立即锁定；否则某个钩子产生 `Done` 后锁定。后续中间件仍执行并看到这一计划，但不能恢复非终止步骤或改写锁定的结束信息。每个钩子返回后检查锁定值；非法修改返回带原因的 `CoreError::Internal`，不静默忽略。后续无修改的观察型钩子不受影响。

这意味着纯文本产生默认 `Done` 后，不能通过 `after_step` 追加模型调用；需要这种特殊流程的用户应覆盖 `forward`。本变更优先采用已确认的终止不可撤销语义，而不是引入“默认结束/强制结束”两种终止类别。

理由：比“最后一个写入者可以撤销停止”更可预测，也保留对错误配置的可见诊断。`after_turn` 不参与下一步选择。

### D5：模型结果先决定调度，再提交完整助手消息

如果先把所有工具调用提交进 `AssistantMessage`，随后 hook 把 `CallTool` 改为 `Done`，就会出现没有结果的历史工具调用。因此模型节点采用：

```text
ModelRequestStarted / 可选提醒 / 读取快照
reasoning_with_hooks：流式增量 + 最终响应 + after_model
计算默认下一步
after_step(Model, &mut next)
验证 next
确定实际保留的调用
记录完整 AssistantMessage / 丢弃调用的 Warning
```

最终 `next == CallTool` 才保留整个合法批次；若选择 `CallModel` 或 `Done`，移除该批次，保留文本，记录被忽略调用数量的 `Warning`。因此不需要生成虚假的已执行工具结果，也不借用失败恢复投影完成正常调度。

`after_step` 收到的是经过 `after_model` 修改的最终模型响应，但此时完整助手消息尚未写入日志；这是为避免悬空调用而确定的事务边界。流式增量已经记录，不能被回滚；复用已有消息 ID 和“最终文本优先，缺失时由增量补齐”规则。

备选：立刻写入工具调用，再为所有跳过调用生成失败结果。它会增加正常调度的工具事件及虚构失败语义，本次不采用。

### D6：跨步骤保留模型迭代边界和预算

`TurnContext` 保留尚未完成的模型迭代及该次用量：

- 模型后选择 `CallTool`：保持模型迭代开放，工具批次及其调度 hook 完成后记录 `ModelRequestCompleted`，然后递增迭代号。
- 模型后选择 `CallModel` 或 `Done`：提交助手消息和警告后立即完成当前模型迭代，再执行下一计划。
- 工具后选择 `Done`：仍先记录对应 `ModelRequestCompleted`，再从 `forward` 返回结果；回合后钩子、`TokenCount`、`TurnComplete` 只执行一次。
- 使用已经过模型后钩子处理的用量累计一次，不因多了一个工具步骤而再次累计。

预算以当前模型调用序号为唯一来源。前 `max_tool_iterations` 次模型调用可以提供工具，随后一次默认禁工具并携带 `ToolLimitReached` 提醒；这次默认以 `ExceedMaxIters` 结束。模型调用总数使用宽整数计算上界 `max_tool_iterations + 1`，实际事件迭代号保持连续并检测溢出。

执行前验证跳转：无批次、批次已消费、预算禁止工具时的 `CallTool` 均报错；超预算的 `CallModel` 在请求模型前报错。模型前钩子也不能通过重新添加工具规格绕过禁工具预算，验证发生在调用模型之前。禁工具响应中的工具调用沿用忽略及警告行为，不能被调度 hook 恢复执行。

默认 `Done` 的结束原因保留现有行为；工具结果后的 hook 可显式正常提前结束。非法调度和 hook 错误沿用线程边界 `Error`，不记录成功完成事件；未完成回合仍由既有 history 投影修复。

### D7：公开类型和变更衔接

`TurnStep`、`StepOutput`、`TurnOutcome` 都属于 `tapestry-core`，通过 `agent/mod.rs`、crate 根导出；不增加协议层依赖。`Middleware` 默认方法仍支持只实现一个 hook 的实现者；更新受影响的方法签名、导入、工厂和测试。

本次规格里的新能力名不是新造概念，而是沿用尚未归档变更。归档时，先合并较早能力的原始要求，再以本变更替换同步策略接口、后置逆序和较旧回合前钩子位置，或显式整合这些差异后再归档；不能让较旧变更最后覆盖新契约。提案阶段不擅自改动那些历史 artifacts。

## Risks / Trade-offs

- [步骤签名、前置回合签名和后置顺序改变] → 标记为 breaking，迁移所有仓库内实现，文档给出原回调到默认循环/调度 hook 的映射。
- [正常提前结束导致悬空调用] → 在调度确定后才提交完整助手消息，增加跳过工具后下一轮请求及恢复 history 的测试。
- [拆成两个步骤意外提前结束模型迭代] → 把开放迭代显式保存在上下文，断言模型开始/助手/工具/模型完成的精确顺序。
- [批次内某个工具已经足够但后续工具仍有副作用] → 明确批次不可正常中断，调度 hook 只能在整批结束后决策，不承诺单工具早停。
- [Done 锁定使部分重试策略不能通过调度 hook 实现] → 明确默认 Done 也不可撤销，特殊流程覆盖 `forward`，不引入隐式重新开启。
- [最终响应改写无法隐藏已发出的原文增量] → 保持既有行为，在文档和测试中明确不是内容审查机制。
- [自定义流程可能绕过默认助手] → 提供并使用统一的步骤/迭代收尾方法，文档明确自定义 `forward` 的义务，不承诺对任意自定义代码强制隔离。
- [未归档规格之间存在历史冲突] → 记录本次契约的优先级及归档衔接，实施时不回退当前工作区，不重写历史提案。

## Migration Plan

1. 在当前工作区基础上引入计划、步骤结果和回合结果契约，更新公开导出及中间件默认入口。
2. 重接执行包装器，迁移默认循环、`ReactAgent` 和调用点；删除同步状态机回调、双路径标志及重复执行覆盖点。
3. 更新和新增回归测试，特别覆盖方法覆盖生效、完整批次后停止、所有节点注册顺序、终止锁定、非法跳转、迭代上限及事件顺序。
4. 更新 `docs/architecture.md` 的职责、流程图、hook 顺序和迁移说明；通过格式、检查、测试和 Clippy 后再归档。
5. 不涉及事件数据迁移。回滚实现时需同步回滚新方法签名及调用方；已写入事件仍使用原线格式。
