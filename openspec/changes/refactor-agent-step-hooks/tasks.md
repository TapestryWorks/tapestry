# Tasks

## 1. 建立步骤和中间件契约

- [x] 1.1 在现有工作区基础上为 `TurnStep` 增加批次 `CallTool`，引入 `StepOutput`、`TurnOutcome`，扩展 `TurnContext` 管理初始计划、待执行批次和开放迭代，更新公开导出；以类型及上下文单测验证初始化、批次只能消费一次和回合状态独立。
- [x] 1.2 在 `Middleware` 增加默认无操作 `after_step`，为 `before_turn` 增加初始计划参数，并迁移所有仓库内实现；用只实现 `after_step` 的中间件测试验证其他钩子仍可省略。
- [x] 1.3 实现按注册顺序组合调度计划及 `Done` 锁定校验；单测覆盖非终止覆盖、后续观察型中间件仍执行、默认 Done 锁定、重启或替换终止信息产生明确错误。
- [x] 1.4 将回合、模型、工具的所有后置钩子改为注册顺序，保留 `Skip` 和错误短路语义；通过 A/B 追踪测试验证每个节点的精确调用顺序与数据逐级传递，并更新相关 API 文档。

## 2. 重接模型和工具原始执行点

- [x] 2.1 将现有模型流消费逻辑并入接收完整请求的 `reasoning`，让 `reasoning_with_hooks` 按构建请求、前钩子、`self.reasoning`、后钩子执行，删除重复 `call_model` 覆盖点；回归测试验证用户覆盖实际生效、一次性模型兼容、空增量、流缺少最终响应及最终文本优先规则。
- [x] 2.2 将原始 `acting` 调整为单工具执行并合并重复的 `execute_tool` 覆盖点，使批次 `acting_with_hooks` 对每个调用执行前钩子、开始事件、原始执行、后钩子、结果修正及结束事件；测试验证覆盖生效、参数改写、未知工具、失败、Skip 和错误 ID 修正。
- [x] 2.3 保持工具批次完整有序执行；通过双调用测试验证第一个结果满足结束条件、失败、未找到或 Skip 均不会正常跳过第二个调用，而中间件/存储错误仍立即终止。
- [x] 2.4 更新模型和工具步骤的 rustdoc 及 `docs/architecture.md` 相应职责说明，明确模型不接收 `Recorder`、单工具覆盖与批次包装的区别；核对示例签名并以相关模块测试验证文档所述入口可用。

## 3. 统一默认调度和回合完成

- [x] 3.1 将默认 `forward` 改为唯一的 `CallModel / CallTool / Done` 循环，删除 `next_step`、`on_model_output`、`on_tool_results`、`Phase` 以及带 `hooks: bool` 的双路径；迁移 `ReactAgent` 和工厂测试，用无中间件/无操作中间件回归验证标准 ReAct 行为一致。
- [x] 3.2 在成功模型步骤和完整工具批次后计算默认计划并调用 `after_step`；测试验证模型、双调用批次、最终模型共触发三次调度节点，结果包含数据后钩子的改写且模型/工具共享对应迭代号。
- [x] 3.3 在模型节点最终调度确定后才提交完整助手消息；测试覆盖选择 Done 或另一轮模型时去除未执行调用、保留文本、记录准确忽略数量，并验证下一次模型请求及恢复投影不存在悬空调用。
- [x] 3.4 通过开放迭代状态维护 `ModelRequestStarted` 到 `ModelRequestCompleted` 的边界和单次用量累计；用精确事件序列测试验证工具仍位于对应模型迭代内，整批后提前结束时完成事件和 token 汇总均只记录一次。
- [x] 3.5 让 `forward_with_hooks` 真正调用 `self.forward`，原始流程仅返回 `TurnOutcome`，由包装器执行回合后钩子及完成记录；测试覆盖自定义 forward 生效、初始 Done 零调用、工具结果提供结束文本和完成事件不重复。
- [x] 3.6 校验无待执行批次、重复批次和其他非法跳转，并保留工具预算、禁工具总结及模型调用硬上限；边界测试覆盖 `max_tool_iterations = 0`、总结仍请求工具、钩子重新添加禁用工具、超预算请求与计数溢出处理。
- [x] 3.7 验证错误仍在线程边界关联原提交；测试覆盖 `after_step` 报错、非法撤销 Done 和批次中错误时不执行剩余钩子、不发送成功完成事件，并沿用 history 失败修复。
- [x] 3.8 更新 `docs/architecture.md` 的完整调度图、所有钩子注册顺序、批次后早停示例及旧同步策略迁移说明；明确 Done 不可撤销、forward 不等于禁用中间件，以及后置模型钩子不是流式审查屏障，核对文档与上述回归用例一致。

## 4. 集成验证

- [x] 4.1 运行 `cargo test -p tapestry-core`，确认所有新增规格场景与既有 core 行为测试通过；核对测试覆盖无操作行为、三种步骤、所有钩子顺序、批次早停和非法调度。
- [x] 4.2 运行 `cargo check --workspace`、`cargo test --workspace` 及 `cargo run -p tapestry-core --example minimal_turn`，验证公开接口迁移完整，CLI 和各 adapter 在协议线格式不变的前提下继续工作。
- [x] 4.3 运行 `cargo fmt --all -- --check` 和 `cargo clippy --workspace --all-targets -- -D warnings`，确认格式及 lint 通过且未引入无关修改。
- [x] 4.4 运行 `openspec validate refactor-agent-step-hooks --strict`，核对 proposal、design、三个能力规格及实现一致，确保没有遗漏或未解释的行为变更。

## Workflow follow-up

- 实施完成并经审阅后再归档，不在本次提案生成阶段修改运行时代码。
- 归档前协调 `merge-turn-machine-into-agent` 和 `add-agent-middleware` 对相同能力的未归档规格；根据当时主规格整理 MODIFIED/REMOVED 操作，显式替换旧同步策略、后置逆序和旧回合前节点位置，避免留下冲突要求或被旧变更覆盖。
