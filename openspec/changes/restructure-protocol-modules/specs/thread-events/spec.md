# Spec Delta

## ADDED Requirements

### Requirement: 客户端命令集合
每个提交 SHALL 由提交 ID 和一个客户端命令组成，线格式中命令字段名为 `command`。客户端命令 SHALL 只包含三种：`update_settings`（携带局部设置）、`start_turn` 和 `interrupt`。

#### Scenario: 提交的线格式
- **WHEN** 序列化一个携带 `interrupt` 命令的提交
- **THEN** 输出包含 `id` 和 `command` 字段，`command.type` 为 `interrupt`，且不包含 `op` 字段

#### Scenario: 已删除的命令不可解析
- **WHEN** 反序列化 `command.type` 为 `configure` 或 `thread_settings` 的提交 JSON
- **THEN** 解析失败

### Requirement: 设置命令是唯一的配置入口
线程 SHALL 只通过 `update_settings` 完成配置。线程收到的第一条 `update_settings` SHALL 应用其中的设置、把线程标记为已配置，并发出携带线程 ID 和当前模型名称的 `SessionConfigured`。后续的 `update_settings` 只更新值不为空的字段，不发出确认事件。

#### Scenario: 首次设置完成配置
- **WHEN** 新线程收到 `update_settings { model: "m1", system_prompt: "sys" }`
- **THEN** 线程发出 `model` 为 `m1` 的 `SessionConfigured`，之后的 `start_turn` 可以正常执行

#### Scenario: 后续设置不发确认
- **WHEN** 已配置的线程再收到只包含 `model: "m2"` 的 `update_settings`
- **THEN** 线程模型变为 `m2`，系统提示词保持不变，且不发出任何事件

#### Scenario: 未配置时开始回合
- **WHEN** 线程在收到任何 `update_settings` 之前收到 `start_turn`
- **THEN** 线程发出与该提交关联的 `Error`，history 不变
