# Spec Delta

## MODIFIED Requirements

### Requirement: 基础设施错误终止回合
当事件追加失败或模型调用失败时，系统 SHALL 终止回合并向调用方返回错误；终止后由事件日志投影得到的 history SHALL 保持对模型请求合法。

#### Scenario: 模型调用失败
- **WHEN** 模型调用返回错误
- **THEN** 回合终止，并由线程边界发出带该回合 `turn_id` 的 `Error`；history 中不存在没有对应工具结果的工具调用

#### Scenario: 事件通道关闭
- **WHEN** 事件存储拒绝追加事件
- **THEN** 回合以事件存储错误终止，不再尝试追加后续事件
