# Spec Delta

## MODIFIED Requirements

### Requirement: SystemReminder 事件
系统 SHALL 为每条加入提醒队列的系统提醒记录一条 `SystemReminder` 事件，事件只携带提醒内容，不携带迭代序号。所属回合由信封中的 `turn_id` 表示。提醒 SHALL 在当前回合内、某次模型请求的事件快照生成之前记录，以便该请求送达。同一次请求前的多条提醒 SHALL 按加入队列的顺序记录。

#### Scenario: 超限强制收尾
- **WHEN** 回合达到工具迭代上限并发起强制收尾调用
- **THEN** 该次调用的事件快照中包含一条提醒内容为 `tool_limit_reached` 的 `SystemReminder`，且位于上一个 `ModelRequestCompleted` 之后

#### Scenario: 提醒线格式
- **WHEN** 序列化一条 `tool_limit_reached` 提醒事件的载荷
- **THEN** 结果为 `{"type":"system_reminder","reminder":{"reminder":"tool_limit_reached"}}`，其中没有 `iteration` 字段

#### Scenario: 旧格式提醒事件可解析
- **WHEN** 反序列化带有 `iteration` 字段的旧 `system_reminder` 事件
- **THEN** 解析成功，忽略 `iteration`，提醒内容不变

### Requirement: 成功回合的事件顺序
成功回合 SHALL 以 `TurnStarted` 开始；随后依次为可选的 `EnvironmentContextUpdated` 和 `UserMessage`；每次模型调用由 `ModelRequestStarted` 与 `ModelRequestCompleted` 包围，其间可含 `AssistantMessage` 以及成对的工具调用事件；`SystemReminder` 可出现在 `UserMessage` 之后、`TokenCount` 之前的任意位置，但 MUST NOT 出现在 `AssistantMessageStart` 与对应 `AssistantMessage` 之间，也 MUST NOT 出现在同一工具调用的 `ToolCallBegin` 与 `ToolCallEnd` 之间；最后依次发出汇总 `TokenCount` 与 `TurnComplete`。同一回合的事件 `log_position` MUST 按此顺序递增。

#### Scenario: 含一次工具调用的回合
- **WHEN** 回合包含一次工具调用并最终回答，且未注入环境上下文、未写入提醒
- **THEN** 事件顺序为 `TurnStarted`、`UserMessage`、`ModelRequestStarted`(0)、`AssistantMessage`(工具调用)、`ToolCallBegin`、`ToolCallEnd`、`ModelRequestCompleted`(0)、`ModelRequestStarted`(1)、`AssistantMessage`(文本)、`ModelRequestCompleted`(1)、`TokenCount`、`TurnComplete`

#### Scenario: 事件均与提交关联
- **WHEN** 任一回合事件被发出
- **THEN** 其 `Event.id` 等于触发该回合的提交 ID，信封中的 `turn_id` 等于该回合的 ID

#### Scenario: 提醒不打断配对事件
- **WHEN** 中间件在工具前后钩子中写入提醒
- **THEN** 提醒事件位于该工具的 `ToolCallBegin` 之前或 `ToolCallEnd` 之后，不在两者之间
