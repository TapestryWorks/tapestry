# Spec Delta

## ADDED Requirements

### Requirement: 事件信封
每个事件 SHALL 携带：触发它的提交 ID（`id`）、线程日志位置 `log_position`、`thread_id`、Unix 毫秒时间戳 `timestamp_ms`，以及属于某个回合时的 `turn_id`。`log_position` 由存储分配，从 1 开始且连续递增。`turn_id` 只出现在信封上，事件载荷中 MUST NOT 重复出现；不属于任何回合的事件，信封中 MUST 省略 `turn_id`。

#### Scenario: 回合事件带 turn_id
- **WHEN** 序列化一个回合内的 `ToolCallBegin` 事件
- **THEN** 信封包含 `id`、`log_position`、`thread_id`、`timestamp_ms`、`turn_id`，`msg` 内不包含 `turn_id`

#### Scenario: 非回合事件省略 turn_id
- **WHEN** 序列化 `SessionConfigured` 事件
- **THEN** 信封中不包含 `turn_id` 字段

### Requirement: UserMessage 事件
回合开始后，线程 SHALL 发出一条 `UserMessage`，携带本回合用户消息的 `content` 和可选的 `name`，内容与写入 history 的用户消息一致。

#### Scenario: 带发言者名称
- **WHEN** 客户端提交 `start_turn { content: "hi", name: "alice" }`
- **THEN** 该回合发出 `UserMessage { content: "hi", name: "alice" }`

### Requirement: AssistantMessage 事件
每当一次模型输出产生 history 中的助手条目时，系统 SHALL 发出一条 `AssistantMessage`，携带可选文本和工具调用列表，内容与该助手条目完全一致。强制收尾时被忽略的工具调用 MUST NOT 出现在其中。模型输出既无文本也无工具调用时 MUST NOT 发出。

#### Scenario: 只有工具调用的输出
- **WHEN** 模型输出不含文本但请求了一次工具调用
- **THEN** 发出 `AssistantMessage`，其中没有文本，包含这一次工具调用

#### Scenario: 强制收尾时忽略的工具调用
- **WHEN** 强制收尾调用的输出同时包含文本和工具调用
- **THEN** `AssistantMessage` 只包含文本，工具调用列表为空，并另外发出 `Warning`

### Requirement: EnvironmentContextUpdated 事件
回合向 history 追加环境上下文条目时，系统 SHALL 发出 `EnvironmentContextUpdated`，携带与该条目相同的完整快照。没有追加时 MUST NOT 发出。

#### Scenario: 首个回合注入时间上下文
- **WHEN** 时间上下文开启的线程执行第一个回合
- **THEN** 该回合在 `UserMessage` 之前发出携带日期和时区的 `EnvironmentContextUpdated`

### Requirement: SystemReminder 事件
某次模型请求附带系统提醒时，系统 SHALL 在该次 `ModelRequestStarted` 之后发出 `SystemReminder`，携带迭代序号和提醒内容。

#### Scenario: 超限强制收尾
- **WHEN** 回合达到工具迭代上限并发起强制收尾调用
- **THEN** 对应的 `ModelRequestStarted` 之后紧跟一条提醒内容为 `tool_limit_reached` 的 `SystemReminder`

## MODIFIED Requirements

### Requirement: 成功回合的事件顺序
成功回合 SHALL 以 `TurnStarted` 开始；随后依次为可选的 `EnvironmentContextUpdated` 和 `UserMessage`；每次模型调用由 `ModelRequestStarted` 与 `ModelRequestCompleted` 包围，其间可含可选的 `SystemReminder`、`AssistantMessage` 以及成对的工具调用事件；最后依次发出汇总 `TokenCount` 与 `TurnComplete`。同一回合的事件 `log_position` MUST 按此顺序递增。

#### Scenario: 含一次工具调用的回合
- **WHEN** 回合包含一次工具调用并最终回答，且未注入环境上下文
- **THEN** 事件顺序为 `TurnStarted`、`UserMessage`、`ModelRequestStarted`(0)、`AssistantMessage`(工具调用)、`ToolCallBegin`、`ToolCallEnd`、`ModelRequestCompleted`(0)、`ModelRequestStarted`(1)、`AssistantMessage`(文本)、`ModelRequestCompleted`(1)、`TokenCount`、`TurnComplete`

#### Scenario: 事件均与提交关联
- **WHEN** 任一回合事件被发出
- **THEN** 其 `Event.id` 等于触发该回合的提交 ID，信封中的 `turn_id` 等于该回合的 ID

### Requirement: 模型调用事件
`ModelRequestStarted` SHALL 携带迭代序号和是否允许工具；`ModelRequestCompleted` SHALL 携带迭代序号及该次调用的 token 用量（`usage`）。两者所属回合由信封中的 `turn_id` 表示。

#### Scenario: 强制收尾调用的标识
- **WHEN** 发生超限后的强制收尾调用
- **THEN** 对应 `ModelRequestStarted` 表明不允许工具

#### Scenario: 单次调用用量
- **WHEN** 一次模型调用的响应报告了输入、输出与缓存计数
- **THEN** 对应 `ModelRequestCompleted.usage` 的四个计数与该响应一致

### Requirement: Warning 事件
系统 SHALL 提供不终止回合的 `Warning` 事件，携带说明文本；与回合相关时，所属回合由信封中的 `turn_id` 表示。

#### Scenario: 收尾阶段忽略工具调用
- **WHEN** 强制收尾调用返回工具调用
- **THEN** 发出 `Warning` 且回合仍以 `TurnComplete` 结束

### Requirement: 事件序列化往返
所有新增和变更的事件类型 MUST 使用内部标签的 snake_case 表示，且序列化后再反序列化得到相等的值；新增字段在缺失时 MUST 取默认值。

#### Scenario: 新事件往返
- **WHEN** 序列化并反序列化 `ModelRequestStarted`、含 `usage` 的 `ModelRequestCompleted`、`Warning`、含工具结果的 `ToolCallEnd`、含 `finish_reason` 的 `TurnComplete`、含 `usage` 的 `TokenCount`、`UserMessage`、`AssistantMessage`、`EnvironmentContextUpdated`、`SystemReminder`，以及带完整信封的 `Event`
- **THEN** 结果与原值相等

### Requirement: TokenCount 汇总回合用量
回合结束前发出的 `TokenCount` SHALL 携带本回合所有模型调用用量的逐项之和（`usage`），所属回合由信封中的 `turn_id` 表示。

#### Scenario: 多次调用的汇总
- **WHEN** 一个回合包含两次模型调用，用量分别为 `input=10, cache_read=5` 和 `input=3, output=7`
- **THEN** `TokenCount.usage` 为 `input=13, output=7, cache_read=5, cache_write=0`，序列化后的 `total_tokens` 为 25

## REMOVED Requirements

### Requirement: AgentMessage 表示完整助手消息
**Reason**: `AgentMessage` 只携带文本，无法表达助手条目中的工具调用，history 不能由它推导。
**Migration**: 改为消费 `AssistantMessage`，读取其中的可选文本；工具调用列表可忽略。
