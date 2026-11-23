# Tasks

## 1. 协议层

- [x] 1.1 删除 `SystemReminderEvent.iteration`，更新 `system_reminder_event_wire_format` 测试，新增“旧格式带 `iteration` 仍可解析”的测试；更新 `SystemReminder` 和事件的过时注释。验证：`cargo test -p tapestry-protocol` 通过
- [x] 1.2 将 `HistoryItem` 改名为 `ConversationItem`（构造函数、re-export、序列化测试；serde tag 不变），修改协议 crate 内所有引用。验证：`cargo test -p tapestry-protocol` 通过，`type` 标签的线格式测试保持不变
- [x] 1.3 修复 `tapestry-adapter-agui`、`tapestry-adapter-a2a`、`tapestry-adapter-cli` 中 `SystemReminderEvent` 的构造与断言。验证：`cargo test -p tapestry-adapter-agui -p tapestry-adapter-a2a -p tapestry-adapter-cli` 通过

## 2. 会话投影与 formatter

- [x] 2.1 将 `HistoryProjection` 改名为 `ConversationProjection`，`history()` 改为 `messages()`；`Recorder::history()` 改为 `messages()`，同步更新 `projection.rs` re-export、`lib.rs`，以及 `thread.rs`、`agent_loop.rs` 等调用方。验证：`cargo check --workspace` 通过
- [x] 2.2 投影把有效提醒单独存放在 `reminders` 中：遇到 `ModelRequestCompleted` 和 `TurnStarted` 时清空，`messages()` 把提醒追加在末尾；简化 `fill_missing_tool_results`，直接在 `items` 末尾追加补全结果。新增单元测试，覆盖 spec 中的场景：工具结果前写入仍送达、请求完成后被消费、空输出也被消费、多条提醒的顺序、跨回合失效、工具执行期间写入、补全结果之后才是提醒，以及增量 `apply` 与 `fold` 结果相等。验证：`cargo test -p tapestry-core formatter::` 通过
- [x] 2.3 删除默认 `Formatter` 中的位置过滤，`events_to_history_items` 改名为 `events_to_items`；替换基于旧过滤规则的测试（例如 `tail 1`/`tail 2`、`old` 系列），并更新 `openai_formatter.rs`、genai `mapping.rs`、mock 模型中的相关测试。验证：`cargo test -p tapestry-core formatter:: model::` 通过

## 3. 提醒队列与 Agent 执行

- [x] 3.1 新增内部类型 `ReminderQueue`：`TurnContext` 通过 `Arc` 持有，提供 `remind()` 和 `flush_reminders()`（逐条记录，返回 `Event`）；`HookCtx` 新增 `remind(&self, r)`，不暴露 `Recorder`。增加单元测试，验证顺序和记录失败时传播 `CoreError::EventStore`。验证：`cargo test -p tapestry-core agent::` 通过
- [x] 3.2 修改 `Middleware` 签名：`before_model(&HookCtx, &[Event])`，`after_model(&HookCtx, &ModelResponse)`；更新 trait 文档注释。验证：`cargo check --workspace` 通过
- [x] 3.3 删除 `Agent::prepare_request` 和 `validate_model_request`，按 design 第 4 节展开 `reasoning_with_hooks`：先记录 `ModelRequestStarted`，再 flush、读取快照、执行 `before_model`、再次 flush 并追加到快照，然后调用 `reasoning`，最后执行 `after_model`。验证：用捕获请求的测试断言 `before_model` 写入的提醒出现在本次快照中且只出现一次
- [x] 3.4 删除 `TurnStep::CallModel.reminder`。默认调度在 `initial_step` 和 `CallTool` 分支预算耗尽时改为 `ctx.remind(ToolLimitReached)`；按 design 第 3 节在 `after_model`（助手消息事件之后）、`before_tool`（`ToolCallBegin` 之前）、`after_tool`（`ToolCallEnd` 之后）、`after_step`、`before_turn` 和 `after_turn`（`TokenCount` 之前）设置 flush 点。验证：新增测试断言提醒事件不出现在 Start/Delta/`AssistantMessage` 之间或 `ToolCallBegin`/`ToolCallEnd` 之间，且 `max_tool_iterations` 为 0 和超限收尾的请求都以 `tool_limit_reached` 结尾
- [x] 3.5 迁移 `agent_loop.rs`、`thread.rs` 中的中间件测试：删除 `prompt_suffix`、`uppercase_response`、`ReenableTools` 等改写场景，改为测试只读观察、模型后钩子不能追加工具调用、多中间件按 a、b 顺序写入提醒、工具钩子提醒在下一次请求中送达、`after_step` 选择 `Done` 时提醒在 `TokenCount` 前记录、提醒记录失败时得到 `Error` 且没有 `TurnComplete`；修改断言“history 包含提醒”的旧测试。验证：`cargo test -p tapestry-core` 通过

## 4. 文档

- [x] 4.1 更新 `docs/architecture.md` 和 AGENTS 说明：提醒的消费与失效规则、有效提醒位于末尾、中间件模型钩子只读并可写入提醒、`ConversationProjection`/`ConversationItem` 命名、`SystemReminder` 事件不再携带迭代序号。验证：`grep -rn "HistoryProjection\|HistoryItem\|prepare_request" docs AGENTS.md crates` 无残留

## 5. 集成检查

- [x] 5.1 运行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`，确认全部通过；运行 `cargo run -p tapestry-core --example minimal_turn`，确认输出正常
