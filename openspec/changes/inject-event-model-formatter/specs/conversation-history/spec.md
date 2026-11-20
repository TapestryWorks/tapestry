# Spec Delta

## MODIFIED Requirements

### Requirement: history 条目类型
history SHALL 只包含以下条目类型：用户消息、助手消息（文本和/或工具调用）、工具结果、环境上下文、系统提醒。工具结果 MUST 与对应工具调用通过调用 ID 关联。系统提醒 SHALL 从事件投影为结构化 history 条目并保留在完整投影中，其是否发送给模型 MUST 由消息转换时的顺序筛选规则决定。

#### Scenario: 工具回合的 history 顺序
- **WHEN** 一个回合包含一次工具调用，且未注入环境上下文
- **THEN** history 依次为：用户消息、含工具调用的助手消息、对应的工具结果、最终助手消息

#### Scenario: 条目类型可区分
- **WHEN** history 同时包含用户消息与环境上下文
- **THEN** 两者的序列化 `type` 标签不同，消费者不需要解析文本内容就能区分

#### Scenario: 系统提醒保留在投影
- **WHEN** 日志包含系统提醒事件，随后出现助手消息
- **THEN** 完整 history 投影仍包含结构化提醒，不因后续消息删除该事实，但发送给模型时该提醒被视为过期

## ADDED Requirements

### Requirement: 仅发送最后一条对话消息之后的提醒
默认格式化策略 SHALL 仅保留最后一个真实用户消息、助手消息或工具结果之后的全部系统提醒，保持其原有顺序，删除其他提醒；没有这些对话条目时 SHALL 保留所有提醒。环境上下文 MUST NOT 构成提醒过期边界，筛选 MUST NOT 删除或重排其他原有历史条目，也 MUST NOT 修改事件日志或完整投影。

#### Scenario: 连续尾部提醒全部保留
- **WHEN** history 为 `(reminder 1, user, assistant, reminder 2, reminder 3)`
- **THEN** 发给模型的消息对应 `(user, assistant, reminder 2, reminder 3)`

#### Scenario: 工具结果使此前提醒过期
- **WHEN** history 为 `(user, reminder 1, tool result, reminder 2)`
- **THEN** 发给模型的消息对应 `(user, tool result, reminder 2)`

#### Scenario: 后续用户或助手消息使提醒过期
- **WHEN** 一条提醒后面出现真实用户消息或助手消息，且该消息之后没有提醒
- **THEN** 发给模型的消息不包含任何提醒

#### Scenario: 没有对话消息
- **WHEN** history 为空，或只包含提醒与环境上下文
- **THEN** 空 history 生成空消息列表；否则所有条目按原顺序发送

#### Scenario: 环境上下文不使提醒过期
- **WHEN** history 为 `(user, reminder 1, environment context, reminder 2)`
- **THEN** 两条提醒均保留，环境上下文保持原位置

### Requirement: 格式化复用工具结果补全
默认格式化策略 SHALL 复用共享补全规则，为异常结束、被后续回合取代或日志截断的回合中缺少结果的工具调用生成失败工具结果，以保持模型输入合法。补全 MUST 保留调用 ID，MUST NOT 覆盖已有结果、重复补全或追加补偿事件。合成结果 MUST NOT 使依据真实历史有效的提醒过期。

#### Scenario: 失败回合的部分工具结果
- **WHEN** 助手消息包含两次工具调用，仅第一次有工具结果，随后出现回合错误
- **THEN** 模型消息保留第一个结果，并为第二个调用生成一个关联 ID 相同的失败结果

#### Scenario: 截断日志补全
- **WHEN** 待格式化的事件快照在带工具调用的助手消息之后结束，未包含对应结果
- **THEN** 消息转换为每个缺少结果的调用补一条失败结果，重复转换相同输入的输出一致

#### Scenario: 合成结果不清除有效提醒
- **WHEN** 真实历史为 `(user, assistant with unfinished tool call, reminder)` 且需要补全工具结果
- **THEN** 模型消息包含对应失败工具结果，并仍以该有效提醒结尾

#### Scenario: 正常工具循环不产生额外失败结果
- **WHEN** 活跃回合的每个工具调用均已有真实结果，然后发起下一次模型请求
- **THEN** 格式化不生成任何额外失败结果，也不改变真实工具结果顺序

#### Scenario: 不同提供方复用补全
- **WHEN** 同一份包含未完成工具调用的事件快照分别发送给默认 Chat 和 Responses 模型
- **THEN** 两者均包含相同关联 ID 的补全失败结果，原始事件日志保持不变
