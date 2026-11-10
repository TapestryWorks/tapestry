# protocol-layers Specification

## Purpose

规定协议 crate 维护所有跨层数据契约的职责，包括前端协议、模型统一协议、工具协议、ReAct 内部消息和 history，以及各协议层之间的依赖边界，使同一概念只有一份权威定义。

## Requirements

### Requirement: 协议类型由协议 crate 唯一维护
凡是在两个组件之间传递的数据契约，都 SHALL 只在协议 crate 中定义。这些组件包括客户端与线程、驱动器与模型客户端、驱动器与工具、驱动器与回合状态机，以及会话 history。核心 crate MUST NOT 再定义与之语义重复的数据类型，只能通过重导出提供它们。

#### Scenario: 核心 crate 不重复定义协议类型
- **WHEN** 审查核心 crate 的公开 API
- **THEN** 模型请求与响应、工具结果、回合步骤、token 用量和 history 条目均来自协议 crate，核心 crate 中不存在字段语义相同的另一份定义

### Requirement: 协议按层组织
协议 crate SHALL 按层把类型划分到独立模块：公共类型、工具协议、history、模型统一协议、ReAct 内部消息和前端协议。crate 根部 MUST 重导出各层的公开类型。

#### Scenario: 从 crate 根部使用协议类型
- **WHEN** 下游代码只导入协议 crate 根部的公开类型
- **THEN** 可以使用任一层的公开类型，无需了解其所在模块

### Requirement: 协议层之间的依赖方向
前端协议层 MUST NOT 依赖 ReAct 内部消息层。协议 crate 整体 MUST NOT 依赖核心 crate。

#### Scenario: 前端协议不暴露内部消息
- **WHEN** 检查前端协议中的提交与事件类型
- **THEN** 其字段不引用任何 ReAct 内部消息类型

#### Scenario: 协议 crate 依赖审查
- **WHEN** 检查协议 crate 的依赖
- **THEN** 不包含核心 crate

### Requirement: 协议只承载结构而不承载策略
协议 crate SHALL 只定义数据结构及其序列化规则。提示词文本渲染、迭代上限判断等策略和 IO 行为 MUST 在核心 crate 中实现。

#### Scenario: 结构化条目的文本由核心渲染
- **WHEN** 一个结构化的 history 条目需要以文本形式发送给模型
- **THEN** 文本由核心 crate 生成，协议 crate 中的该类型不包含提示词文本

### Requirement: 协议类型支持序列化往返
前端协议、模型统一协议、工具协议、history 和 ReAct 内部消息的公开类型 MUST 支持序列化，序列化后再反序列化 SHALL 得到相等的值。带变体的枚举 SHALL 使用 snake_case 的内部标签表示。

#### Scenario: 内部消息往返
- **WHEN** 序列化并反序列化各类回合步骤
- **THEN** 结果与原值相等
