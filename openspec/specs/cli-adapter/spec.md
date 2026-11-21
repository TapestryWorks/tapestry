# cli-adapter Specification

## Purpose
定义 Tapestry 事件转换为命令行输出的规则，使命令行使用者只看到工具调用、工具结果和最终助手消息，并以简洁的人类可读文本呈现。

## Requirements

### Requirement: 默认只输出工具调用、工具结果和助手消息
命令行 adapter SHALL 按 `log_position` 顺序逐条处理同一线程的事件。默认配置下，`ToolCallBegin`（工具调用）、`ToolCallEnd`（工具结果）、带非空文本内容的 `AssistantMessage`（助手消息）SHALL 各产生一条输出；内容为空或仅含工具调用的助手消息以及其他所有事件（包括 `AssistantMessageDelta`、生命周期、会话、模型、上下文、用量、诊断事件）MUST 不产生输出。

#### Scenario: 流式文本回合只保留最终助手消息
- **WHEN** 默认配置的 adapter 依次处理 `TurnStarted`、`AssistantMessageStart`、两条 `AssistantMessageDelta`、`AssistantMessage`、`TokenCount`、`TurnComplete`
- **THEN** 只输出一条，渲染为 `<完整文本>`（无前缀）

#### Scenario: 逐类型检查
- **WHEN** 默认配置的 adapter 处理每一种事件类型
- **THEN** 只有 `ToolCallBegin`、`ToolCallEnd` 和带文本的 `AssistantMessage` 各得到一条输出，其余均为空

### Requirement: 输出渲染格式
每条输出 SHALL 渲染为一段文本：工具调用为 `tool call: <工具名>(<参数紧凑 JSON>)`，参数为空对象或 null 时为 `tool call: <工具名>()`；工具结果为 `tool result: <工具名>: <输出>`，状态非 completed 时为 `tool result: <工具名> (<failed 或 not found>): <输出>`；助手消息为内容原文，不加前缀。

#### Scenario: 工具调用
- **WHEN** 渲染工具 `lookup` 以参数 `{"q":"rust"}` 和 `{}` 的两次调用
- **THEN** 分别为 `tool call: lookup({"q":"rust"})` 与 `tool call: lookup()`

#### Scenario: 工具结果
- **WHEN** 渲染 `lookup` 的 completed 结果 `42` 与 failed 结果 `boom`
- **THEN** 分别为 `tool result: lookup: 42` 与 `tool result: lookup (failed): boom`

#### Scenario: 助手消息
- **WHEN** 渲染内容为 `hello world` 的助手消息
- **THEN** 为 `hello world`

### Requirement: 可选包含增量
命令行 adapter SHALL 提供包含增量的配置。启用后，`AssistantMessageDelta` SHALL 额外输出，渲染为增量原文，其他事件的处理与默认配置相同。

#### Scenario: 启用增量
- **WHEN** 启用增量的 adapter 处理增量为 `hello `、`world` 的流式文本回合
- **THEN** 输出依次渲染为 `hello `、`world`、`hello world`

### Requirement: 收尾不产生输出
事件流结束时调用命令行 adapter 的收尾方法 SHALL 返回空输出。

#### Scenario: 收尾
- **WHEN** 处理完任意事件序列后调用收尾方法
- **THEN** 得到空输出

### Requirement: 命令行工具通过 adapter 输出事件
`tapestry-cli` SHALL 只把经默认配置命令行 adapter 产生的输出逐条渲染后按行写到标准输出。提交的终止判定（`SessionConfigured`、`TurnComplete` 成功，`TurnAborted`、`Error` 失败）MUST 基于原始事件，不受过滤影响。

#### Scenario: 带工具调用的回合
- **WHEN** 模型先请求一次工具调用，再以文本回复结束回合
- **THEN** 标准输出依次为 `tool call: ...`、`tool result: ...`、助手文本三行

#### Scenario: 流式文本回合
- **WHEN** 模型以多个文本增量流式返回一条助手消息并完成回合
- **THEN** 标准输出只有一行 `<完整文本>`（无前缀）

### Requirement: 失败信息写入错误输出
由于诊断和生命周期事件不再输出，`tapestry-cli` 在提交以 `Error` 或 `TurnAborted` 结束时 SHALL 以失败状态退出，并在错误输出中包含错误消息（中止时包含中止原因及可选消息）。

#### Scenario: 模型错误
- **WHEN** 提交以 `Error` 事件结束
- **THEN** 标准输出为空，命令失败，错误输出包含 “agent reported an error:” 及错误消息

### Requirement: 未指定 agent 时使用默认系统提示词
`tapestry-cli` 的 `-a/--agent` 参数 SHALL 为可选。省略时 MUST 不读取任何 agent 文件，并使用内置系统提示词 `You are a helpful assistant.`；模型配置 `config.yaml` 仍为必需。

#### Scenario: 省略 agent
- **WHEN** 只提供 `-p` 运行命令行工具
- **THEN** 发送给模型的系统消息为 `You are a helpful assistant.`

#### Scenario: 缺少模型配置
- **WHEN** 省略 agent 且找不到 `config.yaml`
- **THEN** 命令失败并报告未找到配置文件

### Requirement: adapter crate 依赖边界
命令行 adapter crate MUST 只依赖协议 crate 及通用序列化库，MUST NOT 在非测试依赖中引用核心 crate；协议 crate MUST NOT 依赖命令行 adapter crate。

#### Scenario: 依赖审查
- **WHEN** 检查命令行 adapter crate 与协议 crate 的常规依赖
- **THEN** 前者不包含核心 crate，后者不包含任何 adapter crate
