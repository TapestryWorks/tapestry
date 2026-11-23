# Spec Delta

## MODIFIED Requirements

### Requirement: history 条目类型
history SHALL 只包含以下条目类型：用户消息、助手消息（文本和/或工具调用）、工具结果、环境上下文、系统提醒。工具结果 MUST 与对应工具调用通过调用 ID 关联。系统提醒 SHALL 从事件投影为结构化条目，但投影只保留有效（尚未消费且未失效）的提醒；已消费或已失效的提醒仍保留在事件日志中，但 MUST NOT 出现在投影里。

#### Scenario: 工具回合的 history 顺序
- **WHEN** 一个回合包含一次工具调用，且未注入环境上下文
- **THEN** history 依次为：用户消息、含工具调用的助手消息、对应的工具结果、最终助手消息

#### Scenario: 条目类型可区分
- **WHEN** history 同时包含用户消息与环境上下文
- **THEN** 两者的序列化 `type` 标签不同，消费者不需要解析文本内容就能区分

#### Scenario: 已消费提醒不在投影中
- **WHEN** 日志包含一条系统提醒事件，随后完成了一次模型请求
- **THEN** 事件日志仍包含该提醒事件，但投影和发给模型的消息都不包含该提醒

## REMOVED Requirements

### Requirement: 仅发送最后一条对话消息之后的提醒
**Reason**: 按位置判断提醒是否过期依赖写入时机：提醒写在工具结果之前会被误判为过期；模型输出为空时，同一提醒又会被重复发送。
**Migration**: 改用“系统提醒由下一次完成的模型请求消费”和“新回合使未消费提醒失效”两条规则。自定义 formatter 不应再按位置过滤提醒，直接使用投影结果即可。

## ADDED Requirements

### Requirement: 系统提醒由下一次完成的模型请求消费
系统提醒事件 SHALL 在日志中其后第一个 `ModelRequestCompleted` 出现时视为已消费。在此之前，无论中间记录了多少用户消息、助手消息或工具结果，该提醒都 SHALL 保持有效。同一请求前写入的多条提醒 SHALL 都在该请求中送达，并一起被消费。判断 MUST 仅依据事件日志的顺序，不修改已记录的事件。

#### Scenario: 工具结果之前写入的提醒仍然送达
- **WHEN** 日志依次为 `ModelRequestStarted`、工具调用及提醒、`ToolCallEnd`，此后尚无 `ModelRequestCompleted`
- **THEN** 下一次模型请求的消息包含该提醒

#### Scenario: 请求完成后提醒被消费
- **WHEN** 一条提醒之后出现了 `ModelRequestCompleted`
- **THEN** 后续模型请求的消息不包含该提醒

#### Scenario: 模型输出为空也会消费提醒
- **WHEN** 带提醒的模型请求返回空输出，没有记录助手消息，但记录了 `ModelRequestCompleted`
- **THEN** 下一次模型请求不再包含该提醒

#### Scenario: 多条提醒一起送达
- **WHEN** 在同一次模型请求之前先后写入提醒 A 和 B
- **THEN** 该请求的消息末尾依次为 A、B，请求完成后两者均被消费

### Requirement: 新回合使未消费的提醒失效
投影遇到 `TurnStarted` 时 SHALL 丢弃此前所有尚未消费的系统提醒，使提醒不会跨回合送达。失效 MUST NOT 产生任何补偿事件。

#### Scenario: 失败回合的提醒不带入下一回合
- **WHEN** 回合记录了提醒后，模型请求失败，没有 `ModelRequestCompleted`，随后线程开始新回合
- **THEN** 新回合第一次模型请求的消息不包含旧提醒

### Requirement: 有效提醒位于消息末尾
投影 SHALL 把所有有效提醒按记录顺序放在其他全部条目之后，包括为未完成回合补全的失败工具结果，而不论它们在日志中的位置。因此，提醒 MUST NOT 出现在助手工具调用与其对应工具结果之间。

#### Scenario: 工具执行期间写入的提醒
- **WHEN** 日志依次为含工具调用的助手消息、提醒、对应工具结果
- **THEN** 发给模型的消息依次为助手工具调用、工具结果、提醒

#### Scenario: 补全结果之后才是提醒
- **WHEN** 待格式化的日志为用户消息、含未完成工具调用的助手消息、提醒
- **THEN** 消息依次为用户消息、助手工具调用、补全的失败工具结果、提醒
