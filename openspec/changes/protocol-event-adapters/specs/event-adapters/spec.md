# Spec Delta

## Purpose

定义 Tapestry 事件的分类、过滤，以及转换为外部协议的统一契约，使不同消费方能按需挑选事件，并转换为 AG-UI、A2A 等目标协议，而不用直接处理全部事件类型。

## ADDED Requirements

### Requirement: 每个事件属于唯一类别
每个事件 SHALL 恰好属于以下类别之一：会话（`SessionConfigured`）、生命周期（`TurnStarted`、`TurnComplete`、`TurnAborted`）、消息（`UserMessage` 和全部助手消息事件）、工具（`ToolCallBegin`、`ToolCallEnd`）、模型（`ModelRequestStarted`、`ModelRequestCompleted`）、上下文（`EnvironmentContextUpdated`、`SystemReminder`）、用量（`TokenCount`）、诊断（`Warning`、`Error`）。新增事件类型 MUST 同时确定其类别。

#### Scenario: 查询事件类别
- **WHEN** 查询 `AssistantMessageDelta` 和 `TokenCount` 的类别
- **THEN** 分别为消息和用量

#### Scenario: 类别可序列化
- **WHEN** 序列化并反序列化每一个类别
- **THEN** 结果与原值相等，且以 snake_case 字符串表示

### Requirement: 按类别过滤事件
系统 SHALL 提供由类别集合组成的事件过滤器：事件的类别在集合中时匹配，否则不匹配。系统 SHALL 提供匹配全部类别和不匹配任何类别的过滤器。

#### Scenario: 只要消息与生命周期
- **WHEN** 过滤器只包含消息和生命周期，依次检查 `TurnStarted`、`ToolCallBegin`、`AssistantMessage`、`TokenCount`
- **THEN** 只有 `TurnStarted` 和 `AssistantMessage` 匹配

### Requirement: 协议转换契约
协议 crate SHALL 定义统一的事件转换契约：转换器按 `log_position` 顺序逐条接收同一线程的事件，每条事件产生零到多条目标协议输出，并可以保留跨事件的状态。事件流结束时，调用方 SHALL 调用收尾方法，由转换器输出收尾所需的内容。契约本身 MUST NOT 依赖任何具体协议，也不依赖核心 crate。

#### Scenario: 一个事件产生多条输出
- **WHEN** 一个转换器把一条事件映射为两条输出
- **THEN** 调用方按顺序得到这两条输出

#### Scenario: 默认收尾不输出
- **WHEN** 一个转换器没有自定义收尾行为，调用方调用收尾方法
- **THEN** 得到空输出
