# Design

## Context

- `tapestry-protocol::adapter` 已提供 `EventAdapter`（`adapt` / `finish`）与按类别过滤的 `EventFilter`。需要保留的 `AssistantMessage` 与要丢弃的 `AssistantMessageStart`、`AssistantMessageDelta`、`UserMessage` 同属 `EventCategory::Message`，因此无法仅用 `EventFilter` 表达规则。
- 现有 adapter crate（`tapestry-adapter-agui`、`tapestry-adapter-a2a`）只依赖 `tapestry-protocol` 和 serde，各自导出一个 `XxxAdapter` 类型，一个实例处理一个线程。
- `tapestry-cli::print_events` 对匹配当前提交的事件逐条以 JSON 行输出，并根据事件类型判断提交是否结束。需求见 `specs/cli-adapter/spec.md`。

## Goals / Non-Goals

**Goals:**
- 以与其他 adapter 一致的形态新增 `tapestry-adapter-cli`，承载命令行专用的输出规则。
- 为后续规则（如更多事件过滤、人类可读渲染）预留配置入口，而不改变调用方式。

**Non-Goals:**
- 不在 `EventFilter` 中增加事件类型级别的过滤能力。
- 不新增 CLI 参数开关；包含增量的配置只在库 API 上提供。

## Decisions

1. **输出类型为结构化的 `CliOutput`，通过 `Display` 渲染**
   `CliEventAdapter` 实现 `EventAdapter<Output = CliOutput>`，变体为 `ToolCall { tool_name, arguments }`、`ToolResult { tool_name, status, output }`、`Assistant { content }`、`AssistantDelta { delta }`。渲染格式放在 adapter crate 的 `Display` 实现中，CLI 只负责 `writeln!("{entry}")`。
   - 备选：输出 `String`。结构化输出便于测试和其他前端（如带颜色的终端）复用字段，而默认格式仍集中在一处。
   - 备选：继续输出 `Event` 并在 CLI 内渲染。渲染属于命令行协议处理，应与过滤规则放在同一 crate。
   - 格式选择：工具调用用 `name(args)` 的函数调用形式，一行同时看清工具与参数，空参数省略为 `name()`；工具结果带工具名以便与调用对应，只在非 completed 时标出状态，避免成功路径的噪声；助手消息直接输出原文、不加前缀，作为命令行的主要结果最易读，也便于管道处理。不输出 `ToolCallId`（模型提供方的不透明字符串，对用户无意义）。

2. **配置使用 `CliAdapterOptions` 结构体**
   `CliAdapterOptions { include_deltas: bool }`，`Default` 为 `false`；`CliEventAdapter::new()` 等价于默认配置，`with_options(options)` 自定义。结构体比布尔构造参数更易扩展后续规则。

3. **过滤在 adapter 内按事件变体白名单匹配**
   `adapt` 只为 `ToolCallBegin`、`ToolCallEnd`、`AssistantMessage`（以及 `include_deltas` 时的 `AssistantMessageDelta`）返回单元素 `Vec`，其余返回空；`AssistantMessage` 内容为 `None` 或仅空白时也返回空（仅含工具调用的消息由 `ToolCallBegin` 体现）。白名单优于黑名单：新增事件默认不进入命令行输出，避免意外噪声。`finish` 使用 trait 默认实现（空）。

4. **CLI 中的接入点**
   `run_turn` 为整个线程创建一个 `CliEventAdapter` 并传给 `print_events`；`print_events` 在关联性检查（`event.id == submission`）之后、写出之前调用 `adapter.adapt(&event)`，逐条写出结果。终止判定仍基于原始 `event.msg`，因为终止事件本身被过滤掉了。流结束时调用 `finish` 并写出其结果（当前为空），以符合契约。

5. **失败信息改由错误输出给出**
   `Error`/`TurnAborted` 不再出现在标准输出，CLI 返回的错误分别包含 `ErrorEvent.message`，或中止原因及可选消息，由 `main` 打印到 stderr。

6. **依赖**
   新 crate 常规依赖为 `tapestry-protocol` 与 `serde_json`（工具参数为 `serde_json::Value`）；测试中直接构造 `Event`，不需要 `tapestry-core` dev-dependency。`tapestry-cli` 新增对 `tapestry-adapter-cli` 的 path 依赖。

## Risks / Trade-offs

- [依赖旧 JSON 行输出的脚本失效] → 已在 proposal 标注为 BREAKING；完成/失败改由退出码与 stderr 判断；需要结构化数据的调用方可直接使用 adapter 库或事件订阅。
- [多行内容（工具输出、助手文本）跨越多行打印] → 只在首行加前缀，保持内容原样，便于阅读；不做转义。
- [新增的事件类型被默认忽略] → 白名单为有意选择；协作说明中把 CLI adapter 列入“新增事件需同步检查的 adapter”。
