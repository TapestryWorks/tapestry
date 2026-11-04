# Spec Delta

## MODIFIED Requirements

### Requirement: 提交的新增可选字段
`StartTurn` SHALL 支持可选 `user_name`，`ThreadSettings` SHALL 支持可选 `agent_name` 与可选 `include_time_context`；为 `None` 时序列化 MUST 省略这些字段，缺失时 MUST 能反序列化为 `None`。`include_time_context` 缺失时线程 SHALL 视为开启。

#### Scenario: 旧格式提交可解析
- **WHEN** 反序列化不含 `user_name` 的 `StartTurn` JSON
- **THEN** 成功解析且 `user_name` 为 `None`

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
