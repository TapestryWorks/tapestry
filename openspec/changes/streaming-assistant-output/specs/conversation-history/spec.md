# Spec Delta

## ADDED Requirements

### Requirement: 流式增量不进入 history
history 投影 SHALL 忽略 `AssistantMessageStart` 和 `AssistantMessageDelta`，助手消息条目只由 `AssistantMessage` 产生。只有增量而没有 `AssistantMessage` 的消息 MUST NOT 在 history 中留下任何条目。

#### Scenario: 中途失败的部分文本
- **WHEN** 日志中某条消息有 `AssistantMessageStart` 和若干 `AssistantMessageDelta`，随后是该回合的 `Error`
- **THEN** fold 得到的 history 中没有这条消息的任何文本

#### Scenario: 完整消息只产生一个条目
- **WHEN** 日志中某条消息有三个增量和一条 `AssistantMessage`
- **THEN** fold 得到的 history 中只有一条对应的助手条目，内容等于 `AssistantMessage` 的文本
