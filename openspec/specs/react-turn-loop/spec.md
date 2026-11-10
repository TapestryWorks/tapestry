# react-turn-loop Specification

## Purpose

定义一个回合内“模型 → 工具 → 模型”的 ReAct 循环的可观察行为：何时继续、何时结束、如何处理失败与超限，使消费者可以依赖稳定的终止语义。

## Requirements

### Requirement: 无工具调用时回合完成
当一次模型输出不包含工具调用时，系统 SHALL 结束回合，并以 `completed` 作为结束原因。

#### Scenario: 模型直接回答
- **WHEN** 模型对用户消息返回文本且没有工具调用
- **THEN** 回合以 `completed` 结束，且 `last_agent_message` 等于该文本

### Requirement: 工具调用后继续循环
当模型输出包含工具调用时，系统 SHALL 执行这些调用，将结果加入 history，并再次调用模型，直到模型不再请求工具。

#### Scenario: 单次工具调用后回答
- **WHEN** 模型第一次请求工具，第二次返回文本
- **THEN** 系统共调用模型 2 次，并以 `completed` 结束

#### Scenario: 多个工具调用按顺序执行
- **WHEN** 一次模型输出包含多个工具调用
- **THEN** 系统按调用顺序依次执行，并按相同顺序把结果写入 history

### Requirement: 工具失败回灌而不中断回合
未知工具、工具执行失败都 SHALL 作为工具结果写入 history 并让循环继续，而不是中断回合。

#### Scenario: 模型请求未注册的工具
- **WHEN** 模型请求一个未注册的工具名
- **THEN** 系统生成状态为 `not_found` 的工具结果并回灌给模型，回合继续

#### Scenario: 工具返回失败
- **WHEN** 工具返回状态为 `failed` 的结果
- **THEN** 该结果被写入 history 并回灌给模型，回合继续

### Requirement: 超限后强制收尾
当已达到 `max_tool_iterations` 且模型仍请求工具时，系统 SHALL 再进行一次不提供工具的模型调用，并在该次请求的 history 末尾附加一条 `tool_limit_reached` 系统提醒，要求模型总结。之后系统以 `exceed_max_iters` 正常结束回合，而不是报错。

#### Scenario: 超过迭代上限
- **WHEN** 模型在每次调用中都请求工具并达到上限
- **THEN** 系统再发起一次不带工具的模型调用，回合以 `exceed_max_iters` 结束并发出 `TurnComplete`

#### Scenario: 收尾调用仍请求工具
- **WHEN** 强制收尾的模型调用仍返回工具调用
- **THEN** 系统忽略这些调用，发出 `Warning`，并以 `exceed_max_iters` 结束

#### Scenario: 临时提示不进入 history
- **WHEN** 强制收尾发生
- **THEN** 收尾请求的 history 以 `tool_limit_reached` 系统提醒结尾，而回合结束后的持久 history 中不存在任何系统提醒条目

### Requirement: 基础设施错误终止回合
当事件通道关闭或模型调用失败时，系统 SHALL 终止回合并向调用方返回错误；终止后的 history SHALL 保持对模型请求合法。

#### Scenario: 模型调用失败
- **WHEN** 模型调用返回错误
- **THEN** 回合终止并由线程边界发出 `Error`；history 中不存在没有对应工具结果的工具调用

#### Scenario: 事件通道关闭
- **WHEN** 事件接收端已关闭
- **THEN** 回合以通道关闭错误终止

### Requirement: 回合内不支持中断
在本阶段，运行中的回合 SHALL NOT 被后续提交打断；`Interrupt` 仅作为排队的操作按序处理。

#### Scenario: Interrupt 排队处理
- **WHEN** 回合运行期间收到 `Interrupt`
- **THEN** 该操作在当前回合结束后才被处理，并产生 `interrupted` 原因的 `TurnAborted`
