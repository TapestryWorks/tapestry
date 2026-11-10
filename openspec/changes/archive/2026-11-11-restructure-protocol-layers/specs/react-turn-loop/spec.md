# Spec Delta

## MODIFIED Requirements

### Requirement: 工具失败回灌而不中断回合
未知工具、工具执行失败都 SHALL 作为工具结果写入 history 并让循环继续，而不是中断回合。

#### Scenario: 模型请求未注册的工具
- **WHEN** 模型请求一个未注册的工具名
- **THEN** 系统生成状态为 `not_found` 的工具结果并回灌给模型，回合继续

#### Scenario: 工具返回失败
- **WHEN** 工具返回状态为 `failed` 的结果
- **THEN** 该结果被写入 history 并回灌给模型，回合继续

### Requirement: 超限后强制收尾
当已达到 `max_tool_iterations` 且模型仍请求工具时，系统 SHALL 再进行一次不提供工具的模型调用，并在该次请求的 history 末尾附加一条 `tool_limit_reached` 系统提醒，要求模型总结。之后系统以 `exceed_max_iters` 正常结束回合，而不是报错。

#### Scenario: 超过迭代上限
- **WHEN** 模型在每次调用中都请求工具并达到上限
- **THEN** 系统再发起一次不带工具的模型调用，回合以 `exceed_max_iters` 结束并发出 `TurnComplete`

#### Scenario: 收尾调用仍请求工具
- **WHEN** 强制收尾的模型调用仍返回工具调用
- **THEN** 系统忽略这些调用，发出 `Warning`，并以 `exceed_max_iters` 结束

#### Scenario: 临时提示不进入 history
- **WHEN** 强制收尾发生
- **THEN** 收尾请求的 history 以 `tool_limit_reached` 系统提醒结尾，而回合结束后的持久 history 中不存在任何系统提醒条目
