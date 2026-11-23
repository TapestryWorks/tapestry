# Spec Delta

## ADDED Requirements

### Requirement: 调用模型的步骤不携带提醒
调用模型的下一步计划 SHALL 只描述是否允许工具，MUST NOT 携带系统提醒。默认调度和自定义回合流程 SHALL 通过当前回合的提醒队列写入提醒，并遵循与中间件相同的记录时机和消费规则。默认调度在工具预算耗尽后 SHALL 写入一条 `tool_limit_reached` 提醒，再发起不带工具的总结调用。

#### Scenario: 默认调度的超限提醒
- **WHEN** 工具批次结束后达到 `max_tool_iterations`
- **THEN** 默认调度写入 `tool_limit_reached` 提醒，下一次不带工具的模型请求以该提醒结尾

#### Scenario: 调度钩子切换到禁工具调用
- **WHEN** 步骤后钩子把下一步改为不允许工具的模型调用，且未写入提醒
- **THEN** 该次请求不提供工具，也不包含任何新增提醒

#### Scenario: 自定义回合流程写入提醒
- **WHEN** 自定义 Agent 的回合流程在调用模型前加入一条提醒
- **THEN** 该提醒作为 `SystemReminder` 事件记录，并在随后的模型请求中送达一次
