# Tasks

## 1. protocol 中的转换契约

- [x] 1.1 新增 `adapter.rs`，定义 `EventCategory` 和 `EventMsg::category()`（穷尽 match）；单测覆盖每一种事件的类别，以及类别的 snake_case 往返；在 `lib.rs` 中重导出；`cargo test -p tapestry-protocol` 通过
- [x] 1.2 实现 `EventFilter`（`all`/`none`/`with`/`matches`）；单测覆盖“只要消息与生命周期”场景，以及 `all`/`none` 的边界
- [x] 1.3 定义 `EventAdapter` trait（`adapt`，`finish` 默认返回空）；用一个测试用的 adapter 验证“一个事件产生多条输出”和“默认收尾不输出”；更新 `docs/architecture.md`，说明分层（adapter 依赖 event）以及 adapter 的扩展方式

## 2. AG-UI adapter

- [x] 2.1 创建 `crates/tapestry-adapter-agui` 并加入工作区成员，依赖都使用 `{ workspace = true }`；按照 AG-UI 1.0 schema 定义本次用到的事件类型（RUN_*、STEP_*、TEXT_MESSAGE_*、TOOL_CALL_*、CUSTOM），并做 serde 对照测试（`type` 为大写下划线形式、字段为 camelCase、带 `timestamp`）；`cargo test -p tapestry-adapter-agui` 通过
- [x] 2.2 实现 `AguiAdapter` 中 run、step 和文本消息的映射；单测覆盖 agui-adapter spec 中的成功回合首尾、中断、第一次模型调用、流式文本、非流式文本、只有工具调用等场景
- [x] 2.3 实现工具调用映射和 CUSTOM 过滤；单测覆盖“一次工具调用”“默认丢弃用量”“选择输出用量”
- [x] 2.4 实现异常关闭和 `finish`；单测覆盖“增量过程中出错”和“流在 run 中途结束”；编写 crate 文档，标明规范版本和映射表

## 3. A2A adapter

- [x] 3.1 创建 `crates/tapestry-adapter-a2a` 并加入工作区；按照 A2A 1.0 proto 的 JSON 映射定义 `StreamResponse`、`Task`、`TaskStatus`、`TaskState`、`Message`、`Part`、`Artifact`、`TaskStatusUpdateEvent`、`TaskArtifactUpdateEvent`；serde 对照测试验证每条输出恰好有一个顶层键、枚举为 `TASK_STATE_*`/`ROLE_AGENT`、字段为 camelCase；`cargo test -p tapestry-adapter-a2a` 通过
- [x] 3.2 实现 task 和 artifact 映射；单测覆盖回合开始、流式文本（第一块不追加、后续追加、最终覆盖并标记 lastChunk）、非流式文本
- [x] 3.3 实现终态映射和忽略规则；单测覆盖成功完成、失败、终态后忽略后续事件、工具事件不输出、不带 `turn_id` 的 `Error` 不输出
- [x] 3.4 实现 `finish`；单测覆盖“流在回合中途结束”；编写 crate 文档，标明规范版本和映射表

## 4. 端到端验证

- [x] 4.1 在每个 adapter crate 中，以 dev-dependency 方式引入 `tapestry-core`，编写集成测试：用 `MockModelClient` 运行一个带工具调用的回合，把订阅得到的全部事件交给 adapter，断言输出序列的类型顺序与 spec 一致；`cargo test --workspace` 通过
- [x] 4.2 更新 `AGENTS.md` 和 `README.md`：工作区成员、新增事件时必须同时确定类别和各 adapter 的映射

## 5. 集成检查

- [x] 5.1 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace` 全部通过
- [x] 5.2 `openspec validate protocol-event-adapters --strict` 通过

## Workflow follow-up

- 必须在 `event-sourced-thread-history` 和 `streaming-assistant-output` 都归档之后，再归档本 change。
