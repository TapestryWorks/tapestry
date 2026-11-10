# thread-events Specification

## Purpose

定义协议层事件与字段、成功回合的事件顺序契约，以及公开线格式的序列化兼容规则，使客户端可以用 `Event.id` 可靠地关联并渲染一次提交产生的事件。

## Requirements

### Requirement: 成功回合的事件顺序
成功回合 SHALL 以 `TurnStarted` 开始；每次模型调用由 `ModelRequestStarted` 与 `ModelRequestCompleted` 包围，其间可含 `AgentMessage` 及成对的工具调用事件；之后依次发出汇总 `TokenCount` 与 `TurnComplete`。

#### Scenario: 含一次工具调用的回合
- **WHEN** 回合包含一次工具调用并最终回答
- **THEN** 事件顺序为 `TurnStarted`、`ModelRequestStarted`(0)、`ToolCallBegin`、`ToolCallEnd`、`ModelRequestCompleted`(0)、`ModelRequestStarted`(1)、`AgentMessage`、`ModelRequestCompleted`(1)、`TokenCount`、`TurnComplete`

#### Scenario: 事件均与提交关联
- **WHEN** 任一回合事件被发出
- **THEN** 其 `Event.id` 等于触发该回合的提交 ID

### Requirement: 模型调用事件
`ModelRequestStarted` SHALL 携带 `turn_id`、迭代序号和是否允许工具；`ModelRequestCompleted` SHALL 携带 `turn_id`、迭代序号及该次调用的 token 用量（`usage`）。

#### Scenario: 强制收尾调用的标识
- **WHEN** 发生超限后的强制收尾调用
- **THEN** 对应 `ModelRequestStarted` 表明不允许工具

#### Scenario: 单次调用用量
- **WHEN** 一次模型调用的响应报告了输入、输出与缓存计数
- **THEN** 对应 `ModelRequestCompleted.usage` 的四个计数与该响应一致

### Requirement: 回合完成事件携带结束信息
`TurnComplete` SHALL 携带 `finish_reason`（`completed` 或 `exceed_max_iters`）与可选的 `last_agent_message`。

#### Scenario: 超限结束
- **WHEN** 回合因超限强制收尾而结束
- **THEN** `TurnComplete.finish_reason` 为 `exceed_max_iters`，且不会发出 `Error`

### Requirement: AgentMessage 表示完整助手消息
`AgentMessage` SHALL 表示一条已完整产生的助手文本消息；一个回合可发出多条。

#### Scenario: 工具前后均有文本
- **WHEN** 模型在请求工具的同一输出中带有文本，并在后续调用中给出最终回答
- **THEN** 每段文本各发出一条 `AgentMessage`

### Requirement: Warning 事件
系统 SHALL 提供不终止回合的 `Warning` 事件，携带可选 `turn_id` 与说明文本。

#### Scenario: 收尾阶段忽略工具调用
- **WHEN** 强制收尾调用返回工具调用
- **THEN** 发出 `Warning` 且回合仍以 `TurnComplete` 结束

### Requirement: TurnAborted 使用枚举原因
`TurnAborted` SHALL 携带枚举 `reason`（当前取值 `interrupted`）及可选 `message`。

#### Scenario: Interrupt 产生 TurnAborted
- **WHEN** 线程处理 `Interrupt`
- **THEN** 发出 `reason` 为 `interrupted` 的 `TurnAborted`，线格式中 `reason` 为 snake_case 字符串

### Requirement: 提交的新增可选字段
`StartTurn` SHALL 支持可选 `name`，`ThreadSettings` SHALL 支持可选 `agent_name` 与可选 `include_time_context`；为 `None` 时序列化 MUST 省略这些字段，缺失时 MUST 能反序列化为 `None`。`include_time_context` 缺失时线程 SHALL 视为开启。

#### Scenario: 旧格式提交可解析
- **WHEN** 反序列化不含 `name` 的 `StartTurn` JSON
- **THEN** 成功解析且 `name` 为 `None`

#### Scenario: agent_name 局部更新
- **WHEN** 后续 `ThreadSettings` 仅包含 `agent_name`
- **THEN** 只更新 `agent_name`，其他配置保持不变

#### Scenario: include_time_context 局部更新
- **WHEN** 后续 `ThreadSettings` 仅包含 `include_time_context: false`
- **THEN** 只更新该开关，其他配置保持不变

#### Scenario: 旧格式设置可解析
- **WHEN** 反序列化不含 `include_time_context` 的 `ThreadSettings` JSON
- **THEN** 成功解析，字段为 `None`，线程按开启处理

#### Scenario: include_time_context 往返
- **WHEN** 序列化并反序列化 `include_time_context: Some(false)` 的 `ThreadSettings`
- **THEN** 结果与原值相等

### Requirement: 事件序列化往返
所有新增和变更的事件类型 MUST 使用内部标签的 snake_case 表示，且序列化后再反序列化得到相等的值；新增字段在缺失时 MUST 取默认值。

#### Scenario: 新事件往返
- **WHEN** 序列化并反序列化 `ModelRequestStarted`、含 `usage` 的 `ModelRequestCompleted`、`Warning`、含工具结果的 `ToolCallEnd`、含 `finish_reason` 的 `TurnComplete`、含 `usage` 的 `TokenCount`
- **THEN** 结果与原值相等

### Requirement: StartTurn 的提交结构
`StartTurn` SHALL 由必填的 `content`、可选的 `name` 和可选的 `turn_id` 组成。`content` 与 `name` 直接成为本回合用户消息的内容和发言者名称。

#### Scenario: 提交写入用户消息
- **WHEN** 客户端提交 `StartTurn { content: "hi", name: "alice" }`
- **THEN** history 中本回合的用户消息内容为 `hi`、名称为 `alice`

### Requirement: TokenCount 汇总回合用量
回合结束前发出的 `TokenCount` SHALL 携带 `turn_id`，以及本回合所有模型调用用量的逐项之和（`usage`）。

#### Scenario: 多次调用的汇总
- **WHEN** 一个回合包含两次模型调用，用量分别为 `input=10, cache_read=5` 和 `input=3, output=7`
- **THEN** `TokenCount.usage` 为 `input=13, output=7, cache_read=5, cache_write=0`，序列化后的 `total_tokens` 为 25

### Requirement: ToolCallEnd 不再携带 success
`ToolCallEnd` 的结果状态 SHALL 只由工具结果中的 `status` 表达，事件中 MUST NOT 再有单独的 `success` 字段。

#### Scenario: 序列化的结束事件
- **WHEN** 序列化一个 `ToolCallEnd`
- **THEN** 输出中不包含 `success` 字段，工具结果中包含 `status`
