# Spec Delta

## MODIFIED Requirements

### Requirement: 超限后强制收尾
当已达到 `max_tool_iterations` 且模型仍请求工具时，系统 SHALL 再进行一次不提供工具的模型调用，在发起前记录一条 `tool_limit_reached` 系统提醒事件，并将其纳入该次请求的事件快照，要求模型总结。之后系统以 `exceed_max_iters` 正常结束回合，而不是报错。系统 MUST NOT 在事件之外再次手动附加同一提醒。

#### Scenario: 超过迭代上限
- **WHEN** 模型在每次调用中都请求工具并达到上限
- **THEN** 系统再发起一次不带工具的模型调用，回合以 `exceed_max_iters` 结束并发出 `TurnComplete`

#### Scenario: 收尾调用仍请求工具
- **WHEN** 强制收尾的模型调用仍返回工具调用
- **THEN** 系统忽略这些调用，发出 `Warning`，并以 `exceed_max_iters` 结束

#### Scenario: 收尾提示通过事件转换
- **WHEN** 强制收尾发生
- **THEN** 收尾请求的事件快照包含一条此次生成的 `tool_limit_reached` 提醒事件，格式化后的消息以该提醒结尾且不重复，完整 history 投影保留该结构化提醒

#### Scenario: 临时提示不进入 history
- **WHEN** 收尾请求已记录提醒，随后线程开始新回合并记录用户消息
- **THEN** 旧提醒仍保留在事件快照与完整 history 投影中，但不进入新请求发送给模型的 history 消息视图；此处“history”指模型发送视图，替代此前“不进入持久 history”的行为

#### Scenario: 提醒记录失败
- **WHEN** 强制收尾前记录提醒事件失败
- **THEN** 系统终止当前提交，不发起缺少该提醒的模型请求，错误按现有线程边界规则传播
