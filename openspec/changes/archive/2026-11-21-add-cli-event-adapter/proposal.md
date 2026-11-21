# Proposal

## Why

`tapestry-cli` 目前把订阅到的每个事件原样序列化为 JSON 行输出。流式回合中，`AssistantMessageDelta` 会按词产生大量行（完整文本已由 `AssistantMessage` 给出），生命周期、模型、上下文、用量等内部事件也对命令行使用者意义不大，输出因此冗长且难以被脚本消费。命令行真正关心的是工具调用、工具结果和助手消息。命令行输出的筛选规则应像 AG-UI、A2A 一样，作为独立的出站 adapter 维护，而不是散落在 CLI 的打印循环里，便于后续扩展更多命令行专用的转换规则。

## What Changes

- 新增 crate `tapestry-adapter-cli`，提供实现 `EventAdapter` 的 `CliEventAdapter`：把一个线程的 Tapestry 事件转换为命令行输出事件。
  - 默认只输出工具调用（`ToolCallBegin`）、工具结果（`ToolCallEnd`）和助手消息（`AssistantMessage`），其余事件（含 `AssistantMessageDelta`）全部丢弃；并把它们渲染为人类可读的文本行：`tool call: <工具名>(<参数 JSON>)`（参数为空对象或 null 时为 `<工具名>()`）、`tool result: <工具名>: <输出>`（状态非 completed 时为 `tool result: <工具名> (<failed|not found>): <输出>`）、助手消息原文（无前缀）。仅请求工具、没有文本内容的助手消息不输出（工具调用行已经体现）。
  - 提供选项以额外包含 `AssistantMessageDelta`，供需要实时流式输出的调用方使用。
  - `finish` 不产生额外输出。
- `tapestry-cli` 不再输出 JSON 行，而是逐行打印 `CliEventAdapter` 渲染出的文本；终止判定仍基于原始事件。由于 `error`、`turn_aborted` 不再输出，失败时把错误消息/中止原因写入错误输出。
- **BREAKING**：`tapestry-cli` 的标准输出从 JSON 行改为文本行，且只剩工具调用、工具结果和助手消息三类。
- `tapestry-cli` 的 `-a` 改为可选，省略时使用内置系统提示词 `You are a helpful assistant.`。
- 新 crate 只依赖 `tapestry-protocol`（以及 serde_json），不依赖 `tapestry-core`（测试 dev-dependency 除外）。
- 非目标：不新增命令行参数、不改变输出格式（例如人类可读文本渲染），不涉及入站命令转换。

## Capabilities

### New Capabilities

- `cli-adapter`：Tapestry 事件到命令行输出的转换规则（只保留工具调用、工具结果和助手消息），以及 `tapestry-cli` 使用该 adapter 输出事件的行为。

### Modified Capabilities

（无）

## Impact

- 代码：新增 `crates/tapestry-adapter-cli`，注册为根 `Cargo.toml` 工作区成员；修改 `crates/tapestry-cli/Cargo.toml` 与 `src/main.rs`。
- API：仅新增公开类型，`tapestry-protocol` 和 `tapestry-core` 不变。
- 输出：`tapestry-cli` 标准输出只保留三类事件行，失败信息改由错误输出给出。
- 文档：`docs/architecture.md` 的 crate 映射与 adapter 说明、`AGENTS.md`/协作说明中 adapter crate 列表需同步。
- 依赖：不新增外部依赖。
