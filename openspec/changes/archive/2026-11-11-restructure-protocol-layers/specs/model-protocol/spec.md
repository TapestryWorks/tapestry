# Spec Delta

## Purpose

定义系统与任意模型提供方之间的统一请求/响应契约，以及 token 用量的统一语义，使不同提供方的调用结果可以一致地累计、记录和回放。

## ADDED Requirements

### Requirement: 模型请求内容
模型请求 SHALL 携带可选的 system prompt、本次调用的完整 history、可用工具规格和迭代序号。不允许使用工具时，工具规格 MUST 为空。请求中 MUST NOT 有 history 之外的提示旁路。

#### Scenario: 强制收尾请求
- **WHEN** 系统发起超限后的强制收尾调用
- **THEN** 请求的工具规格为空，收尾提示作为 history 中的最后一条条目出现

### Requirement: 模型响应内容
模型响应 SHALL 携带可选的助手文本、按顺序排列的工具调用列表，以及本次调用的 token 用量。

#### Scenario: 同时返回文本与工具调用
- **WHEN** 提供方在一次响应中返回文本和两个工具调用
- **THEN** 模型响应包含该文本，以及按原顺序排列的两个工具调用

### Requirement: 模型请求与响应可序列化
模型请求与模型响应 MUST 支持序列化，序列化后再反序列化 SHALL 得到相等的值。

#### Scenario: 请求往返
- **WHEN** 序列化并反序列化一个包含用户消息、工具调用和工具结果的模型请求
- **THEN** 结果与原值相等

### Requirement: TokenUsage 字段语义
token 用量 SHALL 包含四个互不重叠的非负计数：`input_tokens` 表示未命中缓存的输入，`output_tokens` 表示全部输出（含 reasoning），`cache_read_tokens` 表示从缓存读取的输入，`cache_write_tokens` 表示写入缓存的输入。

#### Scenario: 输出包含推理
- **WHEN** 提供方报告 100 个输出 token，其中 40 个为 reasoning
- **THEN** `output_tokens` 为 100

### Requirement: TokenUsage 总量为派生值
`total_tokens` SHALL 始终等于四个计数之和。序列化时 MUST 输出 `total_tokens`。反序列化时 MUST 忽略输入中的 `total_tokens`，并按四个计数重新计算。

#### Scenario: 序列化包含总量
- **WHEN** 序列化 `input=10, output=5, cache_read=3, cache_write=2` 的用量
- **THEN** 输出中 `total_tokens` 为 20

#### Scenario: 忽略不一致的总量
- **WHEN** 反序列化四个计数之和为 20、但 `total_tokens` 写为 999 的 JSON
- **THEN** 得到的用量总量为 20

### Requirement: 提供方用量归一化
当提供方报告的总输入已包含缓存部分时，模型客户端 SHALL 先扣除缓存读取和缓存写入，再作为 `input_tokens`，结果不得小于 0。提供方未报告的计数 MUST 视为 0。

#### Scenario: 总输入包含缓存读取
- **WHEN** 提供方报告总输入 100，其中缓存读取 60、缓存写入 10
- **THEN** 用量为 `input_tokens=30, cache_read_tokens=60, cache_write_tokens=10`

#### Scenario: 提供方不报告缓存
- **WHEN** 提供方只报告总输入 7 与输出 3
- **THEN** 用量为 `input_tokens=7, output_tokens=3`，两个缓存计数为 0

### Requirement: 工具调用 ID 来自模型提供方
模型客户端 SHALL 原样采用提供方返回的工具调用 ID，并在后续请求中原样回传。只有当提供方未提供 ID 或 ID 为空时，系统才 MUST 生成一个在该会话内唯一的 ID。

#### Scenario: 复用提供方 ID
- **WHEN** 提供方返回 ID 为 `call_abc` 的工具调用
- **THEN** history 中的工具调用与工具结果均使用 `call_abc`，下一次请求回传给提供方的也是 `call_abc`

#### Scenario: 提供方未给出 ID
- **WHEN** 提供方返回的工具调用 ID 为空
- **THEN** 系统为其生成非空且唯一的 ID，开始事件、结束事件和工具结果都使用该 ID
