# Spec Delta

## Purpose

定义可替换的回合策略与驱动器之间的可观察契约。策略只负责同步决策，驱动器负责全部 IO、事件记录和不变量校验，自定义策略可以改变回合流程，但无法破坏事件顺序与 ID 契约。

## ADDED Requirements

### Requirement: 默认策略保持现有行为
未设置自定义策略时，系统 SHALL 使用默认 ReAct 策略，产生的事件序列、history 和结束原因与引入可替换策略之前完全一致。

#### Scenario: 默认策略的 echo 回合
- **WHEN** 未设置自定义策略，用户消息包含 `tool:echo`
- **THEN** 事件类型序列、history 和 `TurnComplete.finish_reason` 与引入可替换策略之前完全一致

#### Scenario: 默认策略达到迭代上限
- **WHEN** 未设置自定义策略，模型每次都请求工具直到超过 `max_tool_iterations`
- **THEN** 行为与 `react-turn-loop` 的超限强制收尾要求一致，回合以 `exceed_max_iters` 结束

### Requirement: 每个回合使用独立的策略实例
系统 SHALL 在每个回合开始时创建一个新的策略实例，并传入当时的线程配置。策略状态 SHALL NOT 跨回合保留。

#### Scenario: 两个回合互不影响
- **WHEN** 同一线程连续执行两个回合，第一个回合的策略记录了迭代计数
- **THEN** 第二个回合的策略从初始状态开始，迭代计数为 0

### Requirement: 策略决定是否继续与如何调用模型
每次迭代前，系统 SHALL 询问策略下一步：调用模型或结束回合。调用模型时，策略决定是否提供工具以及要记录的系统提醒；结束回合时，策略给出结束原因和最终助手消息。

#### Scenario: 策略在无工具调用时强制再跑一轮
- **WHEN** 自定义策略在模型第一次返回纯文本后仍要求再调用一次模型
- **THEN** 系统发起第二次模型调用，事件中出现两组 `ModelRequestStarted`/`ModelRequestCompleted`，之后按策略给出的结束原因发送 `TurnComplete`

#### Scenario: 策略要求不提供工具
- **WHEN** 策略要求本次模型调用不提供工具
- **THEN** 发给模型的请求中工具列表为空，`ModelRequestStarted.allow_tools` 为 `false`

### Requirement: 策略只能按序丢弃工具调用
策略处理模型输出时 SHALL 只能保留模型返回的工具调用的一个保序子序列，不能新增、重排或改写调用。在不提供工具的模型调用中，策略 SHALL 不保留任何调用。如果违反，系统 SHALL 以内部错误终止回合。

#### Scenario: 策略丢弃部分调用
- **WHEN** 模型返回两个工具调用，策略只保留第二个
- **THEN** 系统只执行第二个调用，助手消息和 history 中只包含被保留的调用

#### Scenario: 策略伪造调用
- **WHEN** 策略返回了一个不在模型输出中的工具调用
- **THEN** 回合终止，线程边界发出 `Error`，且该调用没有被执行

#### Scenario: 不提供工具时策略仍保留调用
- **WHEN** 本次模型调用不提供工具，模型仍返回工具调用，而策略保留了其中一个
- **THEN** 回合终止，线程边界发出 `Error`，且该调用没有被执行

### Requirement: 被丢弃的调用由驱动器发出警告
当策略丢弃了模型返回的工具调用时，系统 SHALL 在记录助手消息之后发出一条 `Warning`，说明被丢弃的调用数量。策略本身不能记录事件。

#### Scenario: 丢弃调用产生警告
- **WHEN** 策略丢弃了模型返回的 2 个工具调用
- **THEN** 事件日志中在该次 `AssistantMessage` 之后出现一条提及 2 个被忽略调用的 `Warning`

### Requirement: 工具结果由驱动器配对
系统 SHALL 按策略保留的调用顺序执行工具，并把与调用一一配对的结果回灌给策略。每个结果的调用 ID 和工具名 SHALL 等于对应调用的值。

#### Scenario: 结果顺序与调用一致
- **WHEN** 策略保留了三个工具调用
- **THEN** 策略收到的三个结果按调用顺序排列，且每个结果的调用 ID 与对应调用一致

### Requirement: 单回合模型调用次数有硬上限
无论策略如何决策，单个回合的模型调用次数 SHALL NOT 超过 `max_tool_iterations + 1`。策略请求超出上限的模型调用时，系统 SHALL 以内部错误终止回合。

#### Scenario: 策略无限循环
- **WHEN** 自定义策略在每次模型输出后都要求再调用模型
- **THEN** 系统最多发起 `max_tool_iterations + 1` 次模型调用，随后回合终止，线程边界发出 `Error`

### Requirement: 事件契约不受策略影响
无论使用哪种策略，系统 SHALL 保持事件契约：回合以 `TurnStarted` 开始，每次模型调用由 `ModelRequestStarted`/`ModelRequestCompleted` 包围，同一迭代的工具事件位于二者之间，`ToolCallBegin`/`ToolCallEnd` 成对出现，最后依次是 `TokenCount` 和 `TurnComplete`。

#### Scenario: 自定义策略下的事件顺序
- **WHEN** 自定义策略执行了一次带工具调用的迭代和一次纯文本迭代后结束
- **THEN** 事件顺序为 `TurnStarted`、`ModelRequestStarted`(0)、助手消息事件、`ToolCallBegin`、`ToolCallEnd`、`ModelRequestCompleted`(0)、`ModelRequestStarted`(1)、助手消息事件、`ModelRequestCompleted`(1)、`TokenCount`、`TurnComplete`
