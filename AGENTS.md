# Tapestry 智能体协作说明

## 构建、测试与检查

所有命令均在工作区根目录运行。

```bash
# 检查整个工作区能否构建
cargo check --workspace

# 运行全部测试
cargo test --workspace

# 运行单个 crate 的测试
cargo test -p tapestry-core
cargo test -p tapestry-protocol

# 运行单个测试；完整路径可避免匹配到名称相似的测试
cargo test -p tapestry-core thread::tests::thread_configure_and_turn -- --exact

# 运行使用模拟模型的端到端示例
cargo run -p tapestry-core --example minimal_turn

# 应用格式化；CI 使用仅检查模式
cargo fmt --all
cargo fmt --all -- --check

# 与 CI 完全一致的 lint 命令
cargo clippy --workspace --all-targets -- -D warnings
```

CI 会在向 `main` 分支推送以及创建拉取请求时，使用 Rust stable 依次执行格式检查、将警告视为错误的 Clippy 检查，以及整个工作区的测试。仓库没有固定 Rust 工具链版本的配置文件。当前工作区仅包含库 crate，因此 `.gitignore` 会忽略 `Cargo.lock`。

## 架构

Tapestry 是一个采用 Codex SQ/EQ 设计的异步智能体内核：

- `tapestry-protocol` 是依赖较少的公开通信协议层，包含客户端操作（`Submission`、`Op`）、输出事件（`Event`、`EventMsg`）、基于 UUID 的 ID，以及用于局部更新的 `ThreadSettings`。面向传输的类型和 serde 行为应放在这里；该 crate 不得依赖 `tapestry-core`。
- `tapestry-core` 负责运行时行为。`TapestryThread::spawn` 会创建有界的 Tokio 提交/事件通道以及长期运行的工作任务；`ThreadHandle` 是客户端提交操作和消费事件的接口。
- 线程工作任务保存会话配置，并按顺序处理提交。执行 `Op::StartTurn` 前，线程必须先收到 `Op::Configure`，或首次收到 `Op::ThreadSettings`。运行时错误统一在线程工作任务边界转换为与原始提交关联的 `EventMsg::Error`。
- `AgentLoop::run_turn` 执行“模型 -> 可选工具 -> 模型”的迭代流程。它会发送生命周期事件、累计每次模型调用的 token 数，并在模型不再返回工具调用或超过 `max_tool_iterations` 时停止。
- `ModelClient` 和 `Tool` 是异步、满足 `Send + Sync` 的扩展点，以 `Arc<dyn ...>` 保存。工具根据 `Tool::name()` 注册到 `ToolRegistry`；模型请求的工具名称必须与注册表键名一致。
- `MockModelClient` 和 `EchoTool` 为本地开发及测试提供确定性行为。用户消息中的 `tool:echo` 等标记可以触发“模型/工具/模型”路径。

`docs/architecture.md` 是架构总览及 Codex 与 Tapestry 模块映射的权威文档。其中提到的 app server、sandbox 和 CLI crate 尚处于规划阶段，不是当前工作区成员。

## 协议与事件约定

- `SubmissionId` 用于将每个事件与触发它的请求关联；`TurnId` 标识逻辑回合；`ToolCallId` 配对工具调用的开始和结束事件。新增操作或事件时必须保持这些 ID 的职责分离。
- 协议枚举使用内部标签的 serde 表示：`#[serde(tag = "type", rename_all = "snake_case")]`。可选的请求和设置字段使用 serde 默认值，并在值为 `None` 时省略。修改公开协议时应保持线格式兼容，并增加 serde 往返测试。
- 新增 ID 类型时使用 `ids.rs` 中私有的 `id_type!` 模式，使其保持为透明 UUID newtype，并具备一致的创建、解析、显示、哈希和 serde 行为。
- 新操作的分发逻辑放在 `thread::process_submission`；回合、模型和工具执行逻辑放在 `AgentLoop`。通过现有事件通道发送事件，并将通道关闭映射为 `CoreError::ChannelClosed`。
- `ThreadSettings` 是局部更新：`ThreadConfig::apply_settings` 只更新值为 `Some` 的字段。第一次提交设置会完成线程配置并发送 `SessionConfigured`；后续设置更新目前不会发送确认事件。
- `ThreadHandle::collect_events_for` 会持续读取共享事件流，直到收到匹配提交的终止事件（`TurnComplete`、`TurnAborted`、`Error` 或 `SessionConfigured`）。返回结果可能包含其他提交的中间事件，因此消费者必须使用 `Event.id` 进行关联。
- 线程工作任务会在提交循环内等待 `AgentLoop::run_turn` 完成，因此操作是串行执行的。如果尚未修改取消机制，不要假设排队的 `Interrupt` 能与正在执行的回合并发运行。

## 代码与测试约定

- 编写或修改代码时，必须同时编写覆盖对应行为的测试用例。新增功能应覆盖主要成功路径及关键边界情况；修复缺陷时必须添加能够复现该问题的回归测试。不得仅修改实现而不提供对应测试。
- 共享依赖版本统一声明在根目录的 `[workspace.dependencies]` 中；成员 crate 使用 `{ workspace = true }` 引用。保持 `tapestry-core` -> `tapestry-protocol` 的单向依赖关系。
- 各 crate 的公开 API 通过其 `lib.rs` 重新导出。新增需要公开的类型时同步更新 re-export。
- 领域错误使用 `thiserror` 枚举（`ProtocolError`、`CoreError`），并通过 `Result` 传播。模型或工具实现不应自行转换协议错误事件；该转换应保留在线程边界。
- 测试放在源码文件内的 `#[cfg(test)] mod tests` 模块中。异步测试使用 `#[tokio::test]`；协议测试验证序列化结构或往返相等性；核心测试使用 `MockModelClient`、内存通道和 `ToolRegistry`。
- 事件顺序属于可观察行为：成功回合以 `TurnStarted` 开始，每次工具调用由匹配的 `ToolCallBegin`/`ToolCallEnd` 包围，最后发送汇总的 `TokenCount` 和 `TurnComplete`。修改生命周期语义或公开操作/事件分类时，应同步更新测试和 `docs/architecture.md`。
