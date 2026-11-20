# Spec Delta

## Purpose

定义把一个 Tapestry 线程的事件转换为 AG-UI 1.0 事件流的映射规则：回合对应 run，助手消息和工具调用对应 AG-UI 的消息事件和工具事件，使 AG-UI 前端无需了解 Tapestry 事件即可渲染 agent 执行过程。

## ADDED Requirements

### Requirement: 回合映射为 run
转换器 SHALL 把回合映射为 AG-UI run：`threadId` 为线程 ID，`runId` 为回合 ID。`TurnStarted` SHALL 产生 `RUN_STARTED`，`TurnComplete` SHALL 产生 `RUN_FINISHED`。`TurnAborted` 和 `Error` SHALL 产生 `RUN_ERROR`，`TurnAborted` 的错误码为 `interrupted`。

#### Scenario: 成功回合的首尾
- **WHEN** 转换一个成功回合的全部事件
- **THEN** 第一条输出是带线程 ID 和回合 ID 的 `RUN_STARTED`，最后一条输出是 `RUN_FINISHED`

#### Scenario: 中断
- **WHEN** 转换一个 `TurnAborted`
- **THEN** 输出 `code` 为 `interrupted` 的 `RUN_ERROR`

### Requirement: 模型调用映射为 step
`ModelRequestStarted` SHALL 产生 `STEP_STARTED`，`ModelRequestCompleted` SHALL 产生 `STEP_FINISHED`。两者的 `stepName` 相同，并且包含迭代序号。

#### Scenario: 第一次模型调用
- **WHEN** 转换迭代 0 的 `ModelRequestStarted` 和 `ModelRequestCompleted`
- **THEN** 依次输出 `stepName` 相同的 `STEP_STARTED` 和 `STEP_FINISHED`

### Requirement: 助手文本映射为文本消息
助手消息有文本时，转换器 SHALL 输出 `TEXT_MESSAGE_START`（`role` 为 `assistant`）、一个或多个 `TEXT_MESSAGE_CONTENT` 和 `TEXT_MESSAGE_END`，`messageId` 均为该消息的 `message_id`。有增量时，每个增量 SHALL 产生一条 CONTENT；没有增量时，SHALL 用完整文本产生一条 CONTENT。没有文本的助手消息 MUST NOT 产生文本消息事件。

#### Scenario: 流式文本
- **WHEN** 转换 `AssistantMessageStart`、两个 `AssistantMessageDelta` 和带文本的 `AssistantMessage`
- **THEN** 输出 `TEXT_MESSAGE_START`、两条 `TEXT_MESSAGE_CONTENT` 和 `TEXT_MESSAGE_END`

#### Scenario: 非流式文本
- **WHEN** 转换 `AssistantMessageStart` 和带文本 `"hi"` 的 `AssistantMessage`，中间没有增量
- **THEN** 输出 `TEXT_MESSAGE_START`、`delta` 为 `"hi"` 的一条 `TEXT_MESSAGE_CONTENT` 和 `TEXT_MESSAGE_END`

#### Scenario: 只有工具调用
- **WHEN** 转换没有文本、只有工具调用的 `AssistantMessage`
- **THEN** 不输出任何 `TEXT_MESSAGE_*` 事件

### Requirement: 工具调用映射
对于 `AssistantMessage` 中的每个工具调用，转换器 SHALL 依次输出 `TOOL_CALL_START`（包含工具调用 ID、工具名称，`parentMessageId` 为该消息 ID）、携带完整 JSON 参数字符串的 `TOOL_CALL_ARGS` 和 `TOOL_CALL_END`。`ToolCallEnd` SHALL 产生 `TOOL_CALL_RESULT`，携带工具调用 ID 和工具输出文本。

#### Scenario: 一次工具调用
- **WHEN** 转换含一个工具调用的 `AssistantMessage`，以及之后对应的 `ToolCallEnd`
- **THEN** 依次输出 `TOOL_CALL_START`、`TOOL_CALL_ARGS`、`TOOL_CALL_END`、`TOOL_CALL_RESULT`，四者的 `toolCallId` 相同

### Requirement: 其他事件可选输出为 CUSTOM
没有直接对应 AG-UI 事件的 Tapestry 事件，默认 SHALL 不产生输出。调用方可以配置一个事件过滤器；被过滤器匹配的这类事件 SHALL 作为 `CUSTOM` 输出，`name` 为 `tapestry.` 加上事件的 snake_case 类型名，`value` 为事件载荷。

#### Scenario: 默认丢弃用量
- **WHEN** 使用默认配置转换 `TokenCount`
- **THEN** 没有输出

#### Scenario: 选择输出用量
- **WHEN** 过滤器包含用量类别，转换 `TokenCount`
- **THEN** 输出 `name` 为 `tapestry.token_count` 的 `CUSTOM`

### Requirement: 异常结束时关闭未完成的消息
回合以 `Error` 或 `TurnAborted` 结束，或者事件流结束（调用收尾方法）时仍有未结束的文本消息时，转换器 SHALL 先为它输出 `TEXT_MESSAGE_END`。事件流结束时如果还有未结束的 run，SHALL 再输出 `RUN_ERROR`。

#### Scenario: 增量过程中出错
- **WHEN** 转换 `AssistantMessageStart`、一个 `AssistantMessageDelta`，随后是该回合的 `Error`
- **THEN** 输出依次包含 `TEXT_MESSAGE_START`、`TEXT_MESSAGE_CONTENT`、`TEXT_MESSAGE_END` 和 `RUN_ERROR`

#### Scenario: 流在 run 中途结束
- **WHEN** 转换 `TurnStarted` 后直接调用收尾方法
- **THEN** 收尾输出一条 `RUN_ERROR`

### Requirement: 输出符合 AG-UI 线格式
转换器的输出 SHALL 序列化为 AG-UI 1.0 的 JSON 事件：`type` 为大写下划线形式的事件类型，字段名为 camelCase，并带有 Unix 毫秒 `timestamp`，取自对应 Tapestry 事件的 `timestamp_ms`。

#### Scenario: RUN_STARTED 的 JSON
- **WHEN** 序列化一条 `RUN_STARTED` 输出
- **THEN** JSON 包含 `"type":"RUN_STARTED"`、`threadId`、`runId` 和 `timestamp`
