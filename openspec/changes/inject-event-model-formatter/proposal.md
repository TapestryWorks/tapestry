# Proposal

## Why

当前 `OpenAiChatModel` 的请求转换绕过了已有 `Formatter`，调用方仍需提供预先投影的 history，无法在初始化模型时选择事件到模型消息的转换策略。将内部事件作为统一请求输入，并由模型持有 formatter，可以集中复用历史投影、系统提醒筛选与工具结果补全，避免运行时和提供方映射各自维护转换规则。

## What Changes

- **BREAKING**：`ModelRequest` 使用完整、有序、截至本次调用的线程 `events` 快照替代 `history`；保留 `system_prompt`、`tools` 和 `iteration`。
- 模型初始化支持注入 `Formatter`，OpenAI Chat、OpenAI Responses 和底层 genai 客户端的流式与非流式请求均使用注入实例进行消息转换。
- `event_to_history_item` 保留 `SystemReminder`，history 投影保留结构化提醒；AgentLoop 不再向请求 history 手动追加提醒。
- formatter 在列表级消息转换中，仅保留最后一个真实 `User`、`Assistant` 或 `ToolResult` 之后的所有提醒；没有这些条目时保留全部提醒。环境上下文不使提醒过期。
- 缺失工具结果的补全继续由 formatter 内的共享逻辑负责，不写入补偿事件，也不让合成工具结果改变提醒有效范围。
- 保留 `HistoryItem` 和运行时的 history 投影，保持事件日志唯一事实来源、工具调用 ID、流式输出和 token 统计语义。

## Capabilities

### New Capabilities

- 无；复用现有模型协议与会话历史能力。

### Modified Capabilities

- `model-protocol`：请求输入改为事件快照，定义模型初始化时的 formatter 注入及两种调用方式的一致转换。
- `conversation-history`：投影保留系统提醒，定义提醒的列表级筛选和 formatter 共享补全规则。
- `react-turn-loop`：强制收尾通过记录提醒事件构造请求，不再维护独立的请求 history 副本。

## Impact

- 协议层：`tapestry-protocol::ModelRequest`、`HistoryItem` 文档及 serde 测试；这是 Rust API 与请求序列化字段的破坏性变更，不提供双输入隐式兼容。
- 核心层：formatter、AgentLoop、Recorder 的事件快照读取、线程恢复与历史投影、mock 模型、Chat/Responses 模型及 genai 请求映射。
- 消费方：相关测试、示例、公开 re-export、README、`docs/architecture.md` 和 `AGENTS.md` 中的旧 history/reminder 约定。
- 无新增外部依赖；协议 crate 继续不依赖 core 或 genai，外部事件 adapter 的线格式与映射不变。
- 与未归档的 `event-sourced-thread-history` 和 `organize-model-and-formatter-modules` 存在前后继关系；本变更明确取代其中“提醒不进入投影”和“仅移动模块、不改变行为”的相关约束，不修改它们的既有产物。
