# Proposal

## Why

现在模型调用只有一次性的 `complete`，助手文本要等整次调用结束才能看到。AG-UI 这类前端协议要求文本按 start/content/end 的顺序增量推送，并用消息 ID 关联；A2A 的 artifact 也支持追加式更新。事件日志已经成为事实来源（见 `event-sourced-thread-history`），因此需要在事件层加入流式增量，同时保证 history 只由完整消息推导，不受增量影响。

## What Changes

- 公共层新增 `MessageId`（UUID 类型，通过 `id_type!` 生成），用于标识一次模型输出所产生的助手消息。
- 新增事件 `AssistantMessageStart { message_id }` 和 `AssistantMessageDelta { message_id, delta }`；`AssistantMessage` 增加 `message_id`。**BREAKING**：`AssistantMessage` 的线格式增加了必填字段。
- 模型统一协议新增流式输出项：零个或多个文本增量，最后以一个完整的 `ModelResponse` 结束。
- `ModelClient` 增加 `stream` 方法，默认实现调用 `complete`，只产生最终结果，因此现有实现无需修改即可继续工作。`AgentLoop` 统一走流式路径。
- `MockModelClient` 实现按词切分的流式输出，便于测试；`GenaiModelClient` 使用 genai 的 `exec_chat_stream` 实现流式输出，并从 stream end 中取得完整内容和 usage。
- history 投影忽略 `AssistantMessageStart` 和 `AssistantMessageDelta`，只使用 `AssistantMessage`。如果回合在增量过程中失败，已推送的部分文本不进入 history。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `thread-events`：新增助手消息的 start/delta 事件和 `message_id`；成功回合的事件顺序加入 start/delta。
- `model-protocol`：新增流式模型输出的契约。
- `conversation-history`：明确增量事件不进入 history。

## Impact

- 代码：`tapestry-protocol` 的 `common/ids.rs`、`event.rs`、`model.rs`；`tapestry-core` 的 `model.rs`（trait 与 Mock）、`agent.rs`、`projection`、`genai/model_client.rs`、`genai/mapping.rs`。
- API：`ModelClient` 增加带默认实现的方法，属于非破坏性变更；事件线格式发生破坏性变更（`AssistantMessage.message_id`）。
- 依赖：genai 流式消费需要 `futures` 的 `StreamExt`。如果工作区还没有，就把 `futures-util` 加入 `[workspace.dependencies]`，只在 `genai` feature 下使用。
- 前置：`event-sourced-thread-history`。
