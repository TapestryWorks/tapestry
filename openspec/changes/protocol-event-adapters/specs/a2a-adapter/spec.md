# Spec Delta

## Purpose

定义把一个 Tapestry 线程的事件转换为 A2A 1.0 流式响应的映射规则：回合对应 task，助手文本对应 artifact，回合结束对应终态状态更新（A2A 1.0 中不再有 final 标记，由终态本身表示结束），使其他 A2A agent 可以把 Tapestry 当作标准的 A2A 任务执行方。

## ADDED Requirements

### Requirement: 回合映射为任务
转换器 SHALL 把回合映射为 A2A task：task ID 为回合 ID，context ID 为线程 ID。`TurnStarted` SHALL 产生一个状态为 working 的 task 快照。

#### Scenario: 回合开始
- **WHEN** 转换 `TurnStarted`
- **THEN** 输出一个 task，其 ID 为回合 ID、context ID 为线程 ID、状态为 working

### Requirement: 助手文本映射为 artifact
有文本的助手消息 SHALL 映射为一个 artifact，artifact ID 为该消息的 `message_id`。每个 `AssistantMessageDelta` SHALL 产生一次 artifact 更新：该消息的第一次更新不追加，之后的更新追加，且都不是最后一块。`AssistantMessage` SHALL 用完整文本产生一次不追加、标记为最后一块的更新。没有文本的助手消息 MUST NOT 产生 artifact 更新。

#### Scenario: 流式文本
- **WHEN** 转换两个 `AssistantMessageDelta` 和带文本的 `AssistantMessage`
- **THEN** 输出三次 artifact 更新：第一次不追加，第二次追加，第三次不追加并标记为最后一块，其中包含完整文本

#### Scenario: 非流式文本
- **WHEN** 转换没有增量、带文本 `"hi"` 的 `AssistantMessage`
- **THEN** 输出一次包含 `"hi"`、不追加并标记为最后一块的 artifact 更新

### Requirement: 回合结束映射为终态
`TurnComplete` SHALL 产生状态为 completed 的状态更新；`TurnAborted` SHALL 产生 canceled 的状态更新；带 `turn_id` 的 `Error` SHALL 产生 failed 的状态更新，状态消息中携带错误文本。task 进入终态后，属于它的后续事件 MUST NOT 再产生输出。

#### Scenario: 成功完成
- **WHEN** 转换 `TurnComplete`
- **THEN** 输出 completed 状态更新

#### Scenario: 终态后忽略后续事件
- **WHEN** 转换 `TurnAborted` 之后，又收到同一回合的 `Error`
- **THEN** `Error` 不产生输出

#### Scenario: 失败
- **WHEN** 转换带 `turn_id`、消息为 `"boom"` 的 `Error`
- **THEN** 输出 failed 状态更新，状态消息中包含 `"boom"`

### Requirement: 无对应概念的事件不输出
不带 `turn_id` 的 `Error`、`SessionConfigured`、`UserMessage`、工具事件、模型事件、上下文事件、用量事件和 `Warning` SHALL 不产生输出。

#### Scenario: 工具事件
- **WHEN** 转换 `ToolCallBegin` 和 `ToolCallEnd`
- **THEN** 没有输出

### Requirement: 流结束时关闭未完成的任务
事件流结束（调用收尾方法）时仍有未到达终态的 task，转换器 SHALL 为它输出 failed 状态更新。

#### Scenario: 流在回合中途结束
- **WHEN** 转换 `TurnStarted` 后直接调用收尾方法
- **THEN** 收尾输出一条 failed 状态更新

### Requirement: 输出符合 A2A 线格式
转换器的输出 SHALL 序列化为 A2A 1.0 的流式响应 JSON，每条输出恰好是 task、状态更新或 artifact 更新三者之一，字段名和枚举取值遵循 A2A 1.0 规范。

#### Scenario: 状态更新的 JSON
- **WHEN** 序列化一条 completed 状态更新
- **THEN** JSON 中只有状态更新一种载荷，包含 task ID、context ID 和 `TASK_STATE_COMPLETED` 状态
