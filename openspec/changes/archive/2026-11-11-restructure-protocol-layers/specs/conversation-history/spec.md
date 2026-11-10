# Spec Delta

## MODIFIED Requirements

### Requirement: history 条目类型
history SHALL 只包含以下条目类型：用户消息、助手消息（文本和/或工具调用）、工具结果、环境上下文、系统提醒。工具结果 MUST 与对应工具调用通过调用 ID 关联。系统提醒 MUST NOT 出现在持久保存的会话 history 中，只能出现在单次模型请求的 history 中。

#### Scenario: 工具回合的 history 顺序
- **WHEN** 一个回合包含一次工具调用，且未注入环境上下文
- **THEN** history 依次为：用户消息、含工具调用的助手消息、对应的工具结果、最终助手消息

#### Scenario: 条目类型可区分
- **WHEN** history 同时包含用户消息与环境上下文
- **THEN** 两者的序列化 `type` 标签不同，消费者不需要解析文本内容就能区分

### Requirement: 用户消息可带发言者名称
用户消息 SHALL 支持可选的 `name`，由 `StartTurn.name` 提供；助手消息 SHALL NOT 携带 `name`。

#### Scenario: StartTurn 指定 user_name
- **WHEN** 客户端提交带 `name` 的 `StartTurn`
- **THEN** 写入 history 的用户消息保留该名称

#### Scenario: 未指定 user_name
- **WHEN** 客户端提交不带 `name` 的 `StartTurn`
- **THEN** 用户消息不含名称

### Requirement: 用户名称基本校验
`StartTurn.name` MUST 非空且不含换行；不满足时系统 SHALL 以错误拒绝该提交，且不改变 history。

#### Scenario: 名称包含换行
- **WHEN** 提交的 `name` 含换行
- **THEN** 线程发出与该提交关联的 `Error`，history 不变

#### Scenario: 名称为空字符串
- **WHEN** 提交的 `name` 为空字符串
- **THEN** 线程发出与该提交关联的 `Error`，history 不变

## ADDED Requirements

### Requirement: 非对话条目在发送时渲染
环境上下文和系统提醒在 history 中 SHALL 以结构化数据保存。只有在向模型发送请求时，才 MUST 把它们渲染为无名称的用户侧文本消息。渲染格式的变化 MUST NOT 要求迁移已保存的 history。

#### Scenario: 系统提醒的发送形式
- **WHEN** 模型请求的 history 末尾是一条系统提醒
- **THEN** 发送给提供方的最后一条消息是不带名称前缀的用户侧文本，内容是该提醒的渲染结果
