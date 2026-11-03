# tool-execution Specification

## Purpose

定义工具对模型的描述方式、执行结果契约以及工具调用事件的配对规则，使工具失败成为模型可观察的信息而不是循环的终止条件。

## Requirements

### Requirement: 工具向模型暴露规格
每个已注册工具 SHALL 提供名称、描述和 JSON Schema 形式的参数说明；模型请求 MUST 携带当前可用工具的规格。

#### Scenario: 请求携带工具规格
- **WHEN** 线程注册了两个工具并发起模型请求
- **THEN** 请求包含这两个工具的规格

#### Scenario: 强制收尾时不携带工具
- **WHEN** 系统进行超限后的强制收尾调用
- **THEN** 该次请求不包含任何工具规格

### Requirement: 工具执行不以错误中断回合
工具执行 SHALL 总是返回结果（输出文本与成功标志）；失败通过成功标志为 false 表达，而不是向循环返回错误。

#### Scenario: 工具失败
- **WHEN** 工具执行失败
- **THEN** 结果的成功标志为 false，输出为失败说明，回合不被中断

### Requirement: 工具调用状态
每个工具调用 SHALL 有唯一调用 ID，以及状态 `completed`、`failed` 或 `not_found` 之一。

#### Scenario: 成功的工具
- **WHEN** 工具返回成功标志为 true
- **THEN** 状态为 `completed`

#### Scenario: 未注册的工具
- **WHEN** 模型请求的工具名不在注册表中
- **THEN** 状态为 `not_found`，输出说明该工具不存在

### Requirement: 调用开始与结束成对
每个工具调用 MUST 先发出 `ToolCallBegin` 再发出具有相同调用 ID 的 `ToolCallEnd`，包括 `failed` 与 `not_found`；`ToolCallEnd` SHALL 携带 `status` 与可选的 `duration_ms`。

#### Scenario: 未知工具的事件
- **WHEN** 模型请求未注册的工具
- **THEN** 事件流中依次出现该调用的 `ToolCallBegin` 和状态为 `not_found` 的 `ToolCallEnd`

#### Scenario: 多个调用的配对
- **WHEN** 一次模型输出包含两个工具调用
- **THEN** 事件按调用顺序各自成对出现，且调用 ID 互不相同
