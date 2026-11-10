# Spec Delta

## MODIFIED Requirements

### Requirement: 工具执行不以错误中断回合
工具 SHALL 接收完整的工具调用，并总是返回统一的工具结果，包括调用 ID、工具名、输出文本和状态。失败通过 `failed` 状态表达，而不是向循环返回错误。

#### Scenario: 工具失败
- **WHEN** 工具执行失败
- **THEN** 结果状态为 `failed`，输出为失败说明，回合不被中断

### Requirement: 工具调用状态
每个工具调用 SHALL 有一个在会话内唯一的调用 ID，以及状态 `completed`、`failed` 或 `not_found` 之一。调用 ID 来源见模型统一协议。

#### Scenario: 成功的工具
- **WHEN** 工具执行成功
- **THEN** 结果状态为 `completed`

#### Scenario: 未注册的工具
- **WHEN** 模型请求的工具名不在注册表中
- **THEN** 系统用同一种工具结果类型表示该结果，状态为 `not_found`，输出说明该工具不存在

### Requirement: 调用开始与结束成对
每个工具调用 MUST 先发出 `ToolCallBegin`，再发出携带相同调用 ID 的 `ToolCallEnd`，`failed` 与 `not_found` 也不例外。`ToolCallEnd` SHALL 携带完整的工具结果与可选的 `duration_ms`。

#### Scenario: 未知工具的事件
- **WHEN** 模型请求未注册的工具
- **THEN** 事件流中依次出现该调用的 `ToolCallBegin`，以及结果状态为 `not_found` 的 `ToolCallEnd`

#### Scenario: 多个调用的配对
- **WHEN** 一次模型输出包含两个工具调用
- **THEN** 事件按调用顺序各自成对出现，且调用 ID 互不相同

## ADDED Requirements

### Requirement: 工具结果与调用对齐
写入 history 和事件的工具结果，其调用 ID 和工具名 MUST 与触发它的工具调用一致。即使工具实现返回了不一致的值，系统也 SHALL 以原始调用为准并修正，而不是中断回合。

#### Scenario: 工具返回错误的调用 ID
- **WHEN** 某个工具实现返回的结果中调用 ID 与原调用不同
- **THEN** history 与 `ToolCallEnd` 中的结果使用原调用的 ID 与工具名，回合继续
