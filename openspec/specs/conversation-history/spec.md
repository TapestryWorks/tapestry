# conversation-history Specification

## Purpose

定义线程级会话 history 的内容、跨回合保留规则，以及多 agent 场景下用 `name` 标识用户侧发言者、用 `agent_name` 告知 agent 自身身份的行为。

## Requirements

### Requirement: history 跨回合保留
线程 SHALL 在会话内保存 history，后续回合的模型请求 MUST 能看到此前回合的用户消息、助手消息和工具结果。

#### Scenario: 第二个回合可见第一个回合
- **WHEN** 同一线程依次执行两个回合
- **THEN** 第二个回合的第一次模型请求包含第一个回合的全部 history 条目，且顺序不变

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

### Requirement: 用户名称渲染为文本前缀
向模型发送请求时，带名称的用户消息 SHALL 渲染为 `"{name}: {content}"`；无名称时 MUST 原样发送内容。名称 SHALL 通过文本前缀表达，而非 provider 的原生 name 字段。每条用户消息独立渲染，连续用户消息不折叠。

#### Scenario: 带名称的用户消息
- **WHEN** 用户消息名称为 `alice`、内容为 `hi`
- **THEN** 发送给模型的该消息文本为 `alice: hi`

#### Scenario: 连续不同发言者
- **WHEN** history 中相邻两条用户消息分别来自 `alice` 与 `bob`
- **THEN** 它们作为两条独立消息发送，各自带前缀

### Requirement: 用户名称基本校验
`StartTurn.name` MUST 非空且不含换行；不满足时系统 SHALL 以错误拒绝该提交，且不改变 history。

#### Scenario: 名称包含换行
- **WHEN** 提交的 `name` 含换行
- **THEN** 线程发出与该提交关联的 `Error`，history 不变

#### Scenario: 名称为空字符串
- **WHEN** 提交的 `name` 为空字符串
- **THEN** 线程发出与该提交关联的 `Error`，history 不变

### Requirement: agent_name 注入 system prompt
当配置了 `agent_name` 时，系统 SHALL 在发送给模型的 system prompt 末尾追加声明 agent 自身名称的一句话；未配置时 MUST NOT 追加。该声明不改变助手消息内容。

#### Scenario: 已配置 agent_name
- **WHEN** 线程设置了 `agent_name` 为 `planner`
- **THEN** 模型请求的 system prompt 以包含 `planner` 的身份声明结尾

#### Scenario: 未配置 agent_name
- **WHEN** 线程未设置 `agent_name`
- **THEN** system prompt 与配置值完全一致

### Requirement: 非对话条目在发送时渲染
环境上下文和系统提醒在 history 中 SHALL 以结构化数据保存。只有在向模型发送请求时，才 MUST 把它们渲染为无名称的用户侧文本消息。渲染格式的变化 MUST NOT 要求迁移已保存的 history。

#### Scenario: 系统提醒的发送形式
- **WHEN** 模型请求的 history 末尾是一条系统提醒
- **THEN** 发送给提供方的最后一条消息是不带名称前缀的用户侧文本，内容是该提醒的渲染结果
