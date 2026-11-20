# Spec Delta

## MODIFIED Requirements

### Requirement: 协议类型由协议 crate 唯一维护
凡是在两个组件之间传递的数据契约，都 SHALL 只在协议 crate 中定义。这些组件包括客户端与线程、驱动器与模型客户端、驱动器与工具，以及会话 history。核心 crate MUST NOT 再定义与之语义重复的数据类型，只能通过重导出提供它们。驱动器与回合状态机之间的内部消息只在核心 crate 内部使用，不属于数据契约，SHALL 由核心 crate 定义。

#### Scenario: 核心 crate 不重复定义协议类型
- **WHEN** 审查核心 crate 的公开 API
- **THEN** 模型请求与响应、工具结果、token 用量和 history 条目均来自协议 crate，核心 crate 中不存在字段语义相同的另一份定义

#### Scenario: 回合步骤由核心 crate 定义
- **WHEN** 审查协议 crate 与核心 crate 的公开 API
- **THEN** 回合步骤和模型输出处理结果只出现在核心 crate 中，协议 crate 中不存在这些类型

### Requirement: 协议按层组织
协议 crate SHALL 按层把类型划分到独立模块：公共类型（含 ID 类型）、工具协议、history、模型统一协议、事件和提交。事件与提交模块 SHALL 位于 crate 根部，而不是嵌套在其他目录中。crate 根部 MUST 重导出各层的公开类型。

#### Scenario: 从 crate 根部使用协议类型
- **WHEN** 下游代码只导入协议 crate 根部的公开类型
- **THEN** 可以使用任一层的公开类型，无需了解其所在模块

#### Scenario: ID 类型属于公共层
- **WHEN** 下游代码通过公共层模块路径导入提交 ID、回合 ID、线程 ID 或工具调用 ID
- **THEN** 导入成功，且这些类型与 crate 根部重导出的类型相同

### Requirement: 协议层之间的依赖方向
事件与提交层 MUST 只依赖公共层和工具层。协议 crate 整体 MUST NOT 依赖核心 crate。

#### Scenario: 前端协议不暴露内部消息
- **WHEN** 检查事件与提交中的类型
- **THEN** 其字段只引用公共层和工具层的类型，不引用任何回合内部消息、history 或模型层类型

#### Scenario: 协议 crate 依赖审查
- **WHEN** 检查协议 crate 的依赖
- **THEN** 不包含核心 crate

### Requirement: 协议类型支持序列化往返
事件、提交、模型统一协议、工具协议和 history 的公开类型 MUST 支持序列化，序列化后再反序列化 SHALL 得到相等的值。带变体的枚举 SHALL 使用 snake_case 的内部标签表示。

#### Scenario: 内部消息往返
- **WHEN** 序列化并反序列化 history 条目、模型请求与响应
- **THEN** 结果与原值相等

#### Scenario: 提交往返
- **WHEN** 序列化并反序列化每一种客户端命令
- **THEN** 结果与原值相等，且命令标签为 snake_case
