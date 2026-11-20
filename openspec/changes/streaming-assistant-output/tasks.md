# Tasks

## 1. 协议类型

- [x] 1.1 在 `common/ids.rs` 中用 `id_type!` 新增 `MessageId`；测试覆盖生成、解析、显示和 serde 往返；`cargo test -p tapestry-protocol` 通过
- [x] 1.2 `event.rs` 新增 `AssistantMessageStart`、`AssistantMessageDelta`，`AssistantMessage` 增加 `message_id`；编写往返测试和 snake_case 标签断言
- [x] 1.3 `model.rs` 新增 `ModelStreamItem`；编写往返测试，验证 `text_delta`/`completed` 标签；`cargo test -p tapestry-protocol` 通过

## 2. 模型客户端

- [x] 2.1 core 新增 `ModelStream`，`ModelClient` 增加带默认实现的 `stream`；单测验证只实现 `complete` 的 fake 客户端通过 `stream` 只得到一个 `Completed`，且内容与 `complete` 一致
- [x] 2.2 `MockModelClient` 实现按词切分的 `stream`；单测验证增量拼接结果等于 `complete` 的文本，且最后一项是 `Completed`
- [x] 2.3 `GenaiModelClient` 实现 `stream`（开启 capture，`Chunk` 映射为 `TextDelta`，`End` 映射为 `Completed`，流提前结束时报错）；在 workspace 中声明 `futures-util` 并在 `genai` feature 下以 optional 方式引入；为映射函数编写单测，用构造的 genai stream 事件验证；`cargo test -p tapestry-core --features genai` 通过

## 3. 驱动器与投影

- [x] 3.1 `AgentLoop` 改用 `stream`，按 design D3 发出 Start、Delta、AssistantMessage 并确定最终文本；测试覆盖 spec 场景：流式文本（同一 `message_id`）、非流式客户端（没有 Delta）、两次输出的 `message_id` 不同、最终内容与增量不一致、最终响应缺少文本时使用增量拼接、空输出不发任何事件；`cargo test -p tapestry-core agent::` 通过
- [x] 3.2 测试流在中途出错的情况：有 Start 和 Delta，没有 `AssistantMessage`，随后是带 `turn_id` 的 `Error`
- [x] 3.3 投影忽略 Start 和 Delta；单测覆盖“中途失败的部分文本”和“完整消息只产生一个条目”；`cargo test -p tapestry-core projection::` 通过
- [x] 3.4 更新“含一次工具调用的回合”的事件顺序测试，加入 `AssistantMessageStart`，并加一个使用流式 Mock 的版本，断言其中有 Delta

## 4. 文档

- [x] 4.1 更新 `docs/architecture.md`：流式事件序列、`ModelClient::stream`、最终文本以最终响应为准的规则、增量不进入 history；更新 `AGENTS.md` 中的事件顺序约定和 ID 职责（`MessageId`）

## 5. 集成检查

- [x] 5.1 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、`cargo test -p tapestry-core --features genai` 全部通过；`cargo run -p tapestry-core --example minimal_turn` 正常运行
- [x] 5.2 `openspec validate streaming-assistant-output --strict` 通过

## Workflow follow-up

- 必须在 `event-sourced-thread-history` 归档之后再归档本 change。
