# Design

## Context

前置条件：`event-sourced-thread-history` 已完成，所有事件都通过 `Recorder::record` 写入日志并应用到投影；`AgentLoop` 调用 `ModelClient::complete` 后会发出 `AssistantMessage`。genai 0.6.5 的 `exec_chat_stream` 依次产生 `Start`、`Chunk(text)`、`ReasoningChunk`、`ToolCallChunk(ToolCall)` 和 `End`；开启 `capture_content`、`capture_tool_calls`、`capture_usage` 之后，`End` 中会带上完整内容、工具调用和 usage。`ToolCallChunk` 给出的是已经解析成 `Value` 的工具调用，不是参数字符串片段。

## Goals / Non-Goals

**Goals:**
- 文本增量能实时出现在事件日志中，并通过同一个 `message_id` 关联到最终消息。
- 不支持流式的模型客户端无需改动。

**Non-Goals:**
- 不提供工具调用参数的增量事件。genai 不提供字符串片段，等有提供方支持时再加。AG-UI adapter 会从完整的 `AssistantMessage` 合成 TOOL_CALL_START/ARGS/END。
- 不提供推理（reasoning/thinking）增量。
- 不对增量做合并或节流，每个提供方 chunk 对应一个事件。
- 不提供关闭流式输出的配置项。

## Decisions

### D1：协议中的流式输出项

`model.rs`：

```rust
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelStreamItem {
    TextDelta { text: String },
    Completed { response: ModelResponse },
}
```

它属于驱动器与模型客户端之间的数据契约，放在 protocol。

### D2：`ModelClient::stream` 带默认实现

```rust
#[async_trait]
pub trait ModelClient: Send + Sync {
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, CoreError>;
    async fn stream(&self, request: ModelRequest) -> Result<ModelStream, CoreError> {
        Ok(ModelStream::completed(self.complete(request).await?))
    }
}
```

`ModelStream` 是 core 中的具体类型，提供 `async fn next(&mut self) -> Option<Result<ModelStreamItem, CoreError>>`，内部为 `Box<dyn ...>` 或 `VecDeque`，做法与 `EventSubscription` 一致，不对外暴露 `futures::Stream`。

- 备选方案是把 trait 改成只有 `stream`。这样所有实现和测试中的 fake 客户端都要改，破坏面大，因此不采用。

### D3：`AssistantMessageStart` 延迟发出

驱动器读取流时：
- 收到第一个 `TextDelta` 时，生成 `MessageId`，发出 `AssistantMessageStart`，再发出 `AssistantMessageDelta`。
- 收到 `Completed` 时：如果之前已经发过 Start，或者最终内容不为空（有文本或工具调用），就补发 Start（如果还没发），然后发出 `AssistantMessage`。如果最终内容为空且没有发过增量，则什么都不发。
- 最终文本按 spec 规则确定：优先使用最终响应中的文本，没有时使用增量拼接的结果。

这样可以保证“发出了 Start 就一定有 AssistantMessage 或 Error”，并延续 (b) 中“空输出不发 AssistantMessage”的规则。

### D4：`MessageId` 由 core 生成

由 core 生成 `MessageId::new()`（UUID，`id_type!`），不使用提供方的消息 ID：并不是所有提供方都有消息 ID，而且 `message_id` 必须在第一个增量时就确定下来。

### D5：genai 流式映射

`GenaiModelClient::stream` 调用 `exec_chat_stream`，并开启三个 capture 选项。`Chunk` 映射为 `TextDelta`；`End` 中的 captured 内容、工具调用和 usage 组装成 `Completed`，复用 `mapping.rs` 中现有的 response 映射逻辑；其余事件忽略。如果流在 `End` 之前结束，返回 `CoreError::Model`。这里需要 `futures::StreamExt`，genai 已经间接依赖 futures，我们在 workspace 中显式声明 `futures-util`，并在 `genai` feature 下以 optional 方式引入。

### D6：MockModelClient

`MockModelClient` 实现 `stream`：把原来 `complete` 返回的文本按空白切分成多个 `TextDelta`，保留分隔符，使拼接结果与原文本一致，最后返回 `Completed`。另外新增一个 `NonStreamingMock`，或在测试里写 fake，用来覆盖默认实现路径。

## Risks / Trade-offs

- [增量事件数量大，日志膨胀] → 内存后端可以接受。持久化后端的 change 可以考虑把增量设为可裁剪，并在信封上增加 transient 标记；本次不做。
- [`AssistantMessage` 线格式增加必填字段] → 尚无外部消费者；补充往返测试。
- [提供方增量与最终文本不一致] → 以最终文本为准，并在 spec 中写明；adapter 在 END 时不应依赖拼接结果做校验。

## Migration Plan

单次源码级迁移，在 `event-sourced-thread-history` 之后实施。回滚方式为 revert。
