# Spec Delta

## MODIFIED Requirements

### Requirement: 无工具调用时回合完成
当一次模型输出不包含可执行工具调用时，默认调度 SHALL 结束回合，并以 `completed` 作为结束原因；超限总结调用仍 SHALL 使用既有的 `exceed_max_iters` 结束原因。模型后钩子 MUST NOT 改变该判断所依据的响应。终止计划 SHALL 不被后续调度钩子重新开启。

#### Scenario: 模型直接回答
- **WHEN** 模型对用户消息返回文本且没有工具调用
- **THEN** 回合以 `completed` 结束，且 `last_agent_message` 等于该文本

#### Scenario: 模型后钩子不能追加工具调用
- **WHEN** 模型原始输出没有工具调用，且注册了模型后钩子
- **THEN** 默认调度仍按原响应结束回合，不执行任何工具

### Requirement: 超限后强制收尾
当已达到 `max_tool_iterations` 且模型仍请求工具时，系统 SHALL 再进行一次不提供工具的模型调用，并在该次请求的事件快照生成之前记录一条 `tool_limit_reached` 系统提醒事件，要求模型总结。之后系统以 `exceed_max_iters` 正常结束回合，而不是报错。系统 MUST NOT 在事件之外再次手动附加同一提醒。

#### Scenario: 超过迭代上限
- **WHEN** 模型在每次调用中都请求工具并达到上限
- **THEN** 系统再发起一次不带工具的模型调用，回合以 `exceed_max_iters` 结束并发出 `TurnComplete`

#### Scenario: 收尾调用仍请求工具
- **WHEN** 强制收尾的模型调用仍返回工具调用
- **THEN** 系统忽略这些调用，发出 `Warning`，并以 `exceed_max_iters` 结束

#### Scenario: 收尾提示通过事件转换
- **WHEN** 强制收尾发生
- **THEN** 收尾请求的事件快照包含一条此次生成的 `tool_limit_reached` 提醒事件，格式化后的消息以该提醒结尾且不重复

#### Scenario: 临时提示不进入 history
- **WHEN** 收尾请求完成后，线程开始新回合并发起模型请求
- **THEN** 旧提醒仍保留在事件日志中，但不出现在投影和新请求的模型消息中

#### Scenario: 提醒记录失败
- **WHEN** 强制收尾前记录提醒事件失败
- **THEN** 系统终止当前提交，不发起缺少该提醒的模型请求，错误按现有线程边界规则传播
