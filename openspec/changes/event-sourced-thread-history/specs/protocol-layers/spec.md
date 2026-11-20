# Spec Delta

## MODIFIED Requirements

### Requirement: 协议按层组织
协议 crate SHALL 按层把类型划分到独立模块：公共类型（含 ID 类型、环境上下文和系统提醒）、工具协议、模型统一协议（含 history 条目）、事件和提交。协议 crate MUST NOT 再有单独的 history 模块。事件与提交模块 SHALL 位于 crate 根部。crate 根部 MUST 重导出各层的公开类型。

#### Scenario: 从 crate 根部使用协议类型
- **WHEN** 下游代码只导入协议 crate 根部的公开类型
- **THEN** 可以使用任一层的公开类型，无需了解其所在模块

#### Scenario: ID 类型属于公共层
- **WHEN** 下游代码通过公共层模块路径导入提交 ID、回合 ID、线程 ID 或工具调用 ID
- **THEN** 导入成功，且这些类型与 crate 根部重导出的类型相同

#### Scenario: 事件和模型请求共用结构化条目
- **WHEN** 事件中的环境上下文、系统提醒与 history 条目中的同名内容比较
- **THEN** 两者是公共层中的同一个类型

### Requirement: 协议层之间的依赖方向
事件与提交层 MUST 只依赖公共层和工具层，MUST NOT 依赖模型层。协议 crate 整体 MUST NOT 依赖核心 crate。

#### Scenario: 前端协议不暴露内部消息
- **WHEN** 检查事件与提交中的类型
- **THEN** 其字段只引用公共层和工具层的类型，不引用 history 条目、模型请求或响应

#### Scenario: 协议 crate 依赖审查
- **WHEN** 检查协议 crate 的依赖
- **THEN** 不包含核心 crate
