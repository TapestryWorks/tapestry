# Spec Delta

## ADDED Requirements

### Requirement: 助手消息的流式事件
每次模型输出如果产生助手消息，系统 SHALL 先发出 `AssistantMessageStart`，然后按到达顺序为每段文本增量发出 `AssistantMessageDelta`，最后发出 `AssistantMessage`。这三类事件 MUST 携带同一个 `message_id`。模型客户端不产生增量时，`AssistantMessageStart` 之后 SHALL 直接是 `AssistantMessage`。

#### Scenario: 流式文本
- **WHEN** 模型客户端依次产生增量 `"Hel"`、`"lo"`，最终内容为 `"Hello"`
- **THEN** 依次发出 `AssistantMessageStart`、`AssistantMessageDelta("Hel")`、`AssistantMessageDelta("lo")`、`AssistantMessage("Hello")`，四者的 `message_id` 相同

#### Scenario: 非流式客户端
- **WHEN** 模型客户端只返回最终响应，没有增量
- **THEN** 发出 `AssistantMessageStart` 后紧跟 `AssistantMessage`，中间没有 `AssistantMessageDelta`

#### Scenario: 每次模型输出使用新的消息 ID
- **WHEN** 一个回合中有两次模型输出都产生了助手消息
- **THEN** 两条 `AssistantMessage` 的 `message_id` 不同

### Requirement: 完整消息以最终响应为准
`AssistantMessage` 的文本 SHALL 取自模型客户端的最终响应。最终响应没有文本但已经产生过增量时，`AssistantMessage` 的文本 SHALL 为全部增量按顺序拼接的结果。

#### Scenario: 最终内容与增量不一致
- **WHEN** 增量拼接为 `"Hi"`，而最终响应文本为 `"Hi!"`
- **THEN** `AssistantMessage` 的文本为 `"Hi!"`

#### Scenario: 最终响应缺少文本
- **WHEN** 增量拼接为 `"Hi"`，最终响应没有文本也没有工具调用
- **THEN** 仍然发出文本为 `"Hi"` 的 `AssistantMessage`

### Requirement: 增量过程中失败
模型调用在产生增量之后失败时，系统 SHALL NOT 为该消息发出 `AssistantMessage`，并按基础设施错误终止回合。

#### Scenario: 流在中途出错
- **WHEN** 模型客户端产生一段增量后返回错误
- **THEN** 日志中有该消息的 `AssistantMessageStart` 和 `AssistantMessageDelta`，没有对应的 `AssistantMessage`，随后是该回合的 `Error`

## MODIFIED Requirements

### Requirement: 成功回合的事件顺序
成功回合 SHALL 以 `TurnStarted` 开始；随后依次为可选的 `EnvironmentContextUpdated` 和 `UserMessage`；每次模型调用由 `ModelRequestStarted` 与 `ModelRequestCompleted` 包围，其间可含可选的 `SystemReminder`、一组助手消息事件（`AssistantMessageStart`、零或多个 `AssistantMessageDelta`、`AssistantMessage`）以及成对的工具调用事件；最后依次发出汇总 `TokenCount` 与 `TurnComplete`。同一回合的事件 `log_position` MUST 按此顺序递增。

#### Scenario: 含一次工具调用的回合
- **WHEN** 回合包含一次工具调用并最终回答，未注入环境上下文，且模型客户端不产生增量
- **THEN** 事件顺序为 `TurnStarted`、`UserMessage`、`ModelRequestStarted`(0)、`AssistantMessageStart`、`AssistantMessage`(工具调用)、`ToolCallBegin`、`ToolCallEnd`、`ModelRequestCompleted`(0)、`ModelRequestStarted`(1)、`AssistantMessageStart`、`AssistantMessage`(文本)、`ModelRequestCompleted`(1)、`TokenCount`、`TurnComplete`

#### Scenario: 事件均与提交关联
- **WHEN** 任一回合事件被发出
- **THEN** 其 `Event.id` 等于触发该回合的提交 ID，信封中的 `turn_id` 等于该回合的 ID
