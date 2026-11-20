# Spec Delta

## ADDED Requirements

### Requirement: history 是事件日志的投影
会话 history SHALL 等于按 `log_position` 顺序对线程事件日志 fold 的结果：`UserMessage` 产生用户消息，`AssistantMessage` 产生助手消息，`ToolCallEnd` 产生工具结果，`EnvironmentContextUpdated` 产生环境上下文；其他事件，包括 `SystemReminder`，MUST NOT 产生持久 history 条目。线程 MUST NOT 绕过事件日志直接修改 history。

#### Scenario: 投影与线程 history 一致
- **WHEN** 线程执行若干回合（包括工具调用和时间上下文注入）后，对其完整日志重新 fold
- **THEN** 结果与线程用于下一次模型请求的 history 完全相等

#### Scenario: 系统提醒不进入投影
- **WHEN** 日志中包含 `SystemReminder` 事件
- **THEN** fold 得到的 history 中不包含系统提醒条目

### Requirement: 投影修复未完成的工具调用
一个回合以 `TurnComplete` 之外的方式结束（包括日志在回合中途结束）时，投影 SHALL 为该回合中所有没有对应工具结果的工具调用补上失败状态的工具结果，使 history 对模型请求保持合法。

#### Scenario: 回合中途失败
- **WHEN** 日志中一条 `AssistantMessage` 含两次工具调用，只有第一次有 `ToolCallEnd`，随后是该回合的 `Error`
- **THEN** fold 得到的 history 中第二次调用有一条状态为 `failed` 的工具结果

#### Scenario: 日志在回合中途截断
- **WHEN** 日志最后一个回合在 `AssistantMessage`（含工具调用）之后就结束了
- **THEN** fold 得到的 history 中每个工具调用都有对应的工具结果

## MODIFIED Requirements

### Requirement: history 跨回合保留
线程 SHALL 在会话内保存 history，后续回合的模型请求 MUST 能看到此前回合的用户消息、助手消息和工具结果。history 的保存期限与线程事件日志一致：只要日志仍在，从中恢复的线程也 MUST 能看到这些条目。

#### Scenario: 第二个回合可见第一个回合
- **WHEN** 同一线程依次执行两个回合
- **THEN** 第二个回合的第一次模型请求包含第一个回合的全部 history 条目，且顺序不变
