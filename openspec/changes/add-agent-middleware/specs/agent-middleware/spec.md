# Spec Delta

## Purpose

定义 agent 回合在关键节点（回合开始与结束、模型调用前后、工具调用前后）执行已注册中间件的可观察行为。调用方可以在不破坏核心流程、事件顺序和 ID 契约的前提下观察或改写这些节点的数据。

## ADDED Requirements

### Requirement: 无中间件时行为不变
未注册任何中间件时，系统 SHALL 产生与引入中间件之前完全相同的事件序列和 history。

#### Scenario: 无中间件的 echo 回合
- **WHEN** 未注册中间件的 agent 处理一条包含 `tool:echo` 的用户消息
- **THEN** 事件类型序列和 history 与未引入中间件时完全相同

### Requirement: 回合前后节点
系统 SHALL 在 `TurnStarted` 记录之后、本回合环境上下文和用户消息记录之前，依次调用每个中间件的回合前钩子。回合正常结束时，系统 SHALL 在记录 `TokenCount`、`TurnComplete` 之前调用回合后钩子。

#### Scenario: 回合钩子包围回合
- **WHEN** 注册了一个记录调用的中间件，并完成一个回合
- **THEN** 回合前钩子在本回合所有其他钩子之前调用，回合后钩子在所有其他钩子之后调用，且回合后钩子执行时 `TurnComplete` 尚未记录

#### Scenario: 回合出错时不调用回合后钩子
- **WHEN** 回合因基础设施错误或中间件错误提前结束
- **THEN** 不调用回合后钩子，回合以 `Error` 事件结束

### Requirement: 模型调用前改写请求
系统 SHALL 在每次模型调用前，即读取事件快照并组装请求之后、发送给模型之前，调用模型前钩子。钩子对请求的改写 SHALL 作用于实际发送给模型的请求，且不产生任何事件。

#### Scenario: 中间件改写 system prompt
- **WHEN** 模型前钩子在请求的 system prompt 后追加文本
- **THEN** 模型收到的请求包含追加后的 system prompt，事件日志中没有因此新增的事件

### Requirement: 模型结果后改写响应
系统 SHALL 在每次模型调用得到最终响应之后、记录助手消息并决定下一步之前，调用模型后钩子。改写后的响应 SHALL 决定助手消息内容、后续工具调用和该次请求的用量。已记录的流式增量不受影响，助手消息以改写后的最终内容为准。

#### Scenario: 中间件改写模型文本
- **WHEN** 模型返回文本 `hello`，模型后钩子把它改为 `HELLO`
- **THEN** `AssistantMessage` 内容、history 中的助手条目和 `TurnComplete` 的 `last_agent_message` 都是 `HELLO`

### Requirement: 工具调用前改写参数
系统 SHALL 在每个工具调用执行前、记录 `ToolCallBegin` 之前调用工具前钩子。钩子可以改写调用参数，`ToolCallBegin` 和工具执行 SHALL 使用改写后的参数。调用 ID 和工具名 SHALL 保持模型给出的原值，钩子对它们的改动无效。

#### Scenario: 中间件改写 echo 参数
- **WHEN** 模型请求 `echo` 工具，工具前钩子把参数改为 `{"input":"patched"}`
- **THEN** `ToolCallBegin` 的参数为 `{"input":"patched"}`，工具结果输出为 `patched`

#### Scenario: 钩子改动调用 ID 和工具名无效
- **WHEN** 工具前钩子修改了调用 ID 或工具名
- **THEN** `ToolCallBegin`、`ToolCallEnd` 和工具结果仍使用模型给出的调用 ID 和工具名，并执行原工具

### Requirement: 工具调用前跳过执行
工具前钩子 SHALL 能够以给定结果跳过本次工具执行。跳过时，系统不执行工具，也不再调用其余中间件的工具前钩子，但仍成对记录 `ToolCallBegin` 和 `ToolCallEnd`，并把给定结果作为该调用的结果写入 history。

#### Scenario: 中间件跳过 echo 执行
- **WHEN** 工具前钩子对 `echo` 调用返回跳过，并给出输出 `skipped`
- **THEN** `EchoTool` 不被执行，`ToolCallBegin`/`ToolCallEnd` 成对出现，结果输出为 `skipped`，回合继续调用模型

### Requirement: 工具结果后改写结果
系统 SHALL 在得到工具结果之后、记录 `ToolCallEnd` 之前调用所有中间件的工具后钩子，包括工具未找到和被跳过的情况。改写后的结果 SHALL 被记录并写入 history，其调用 ID 和工具名与调用保持一致。

#### Scenario: 中间件改写 echo 结果
- **WHEN** 工具后钩子把 echo 结果的输出改为 `rewritten`
- **THEN** `ToolCallEnd` 的结果输出为 `rewritten`，下一次模型请求的 history 中该工具结果也是 `rewritten`

#### Scenario: 跳过时仍调用工具后钩子
- **WHEN** 工具前钩子跳过 echo 执行
- **THEN** 所有中间件的工具后钩子仍被调用，并收到跳过时给出的结果

### Requirement: 多中间件执行顺序
系统 SHALL 支持注册零个或多个中间件。前置钩子（回合前、模型前、工具前）按注册顺序执行，后置钩子（回合后、模型后、工具后）按注册逆序执行。每个钩子收到的都是前一个中间件改写后的数据。

#### Scenario: 两个中间件的顺序
- **WHEN** 依次注册中间件 A 和 B，并完成一个包含 echo 调用的回合
- **THEN** 每个节点的前置钩子按 A、B 顺序调用，后置钩子按 B、A 顺序调用

#### Scenario: 改写逐个传递
- **WHEN** A 的工具前钩子把参数改为 `{"input":"a"}`，B 的工具前钩子在收到的 `input` 后追加 `b`
- **THEN** 工具结果输出为 `ab`

### Requirement: 中间件错误终止回合
任一钩子返回错误时，系统 SHALL 立即停止当前回合，不再调用剩余钩子，并像其他运行时错误一样在线程边界记录与原提交关联的 `Error` 事件。

#### Scenario: 模型前钩子报错
- **WHEN** 模型前钩子返回错误
- **THEN** 模型不被调用，回合以与该提交关联的 `Error` 事件结束，且没有 `TurnComplete`

### Requirement: 中间件自有状态
中间件 SHALL 能够在自身保存自定义状态，并在多次钩子调用和多个回合之间读写。状态的生命周期与中间件实例相同，不写入事件日志。

#### Scenario: 跨回合计数
- **WHEN** 一个中间件在每次工具前钩子中递增自身计数器，同一线程完成两个各含一次 echo 调用的回合
- **THEN** 该中间件的计数器为 2，且事件日志中没有与中间件状态相关的事件
