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

CI 会在向 `main` 分支推送以及创建拉取请求时，使用 Rust stable 依次执行格式检查、将警告视为错误的 Clippy 检查，以及整个工作区的测试。仓库没有固定 Rust 工具链版本的配置文件。`.gitignore` 会忽略 `Cargo.lock`。

## 架构

Tapestry 是一个采用 Codex SQ/EQ 设计的异步智能体内核：

- `tapestry-protocol` 是依赖较少的内部协议层，维护 Tapestry 数据契约，并按层划分模块：`common`（含 `common::ids`、`common::context`）← `tool` ← `model`（含 `HistoryItem`），`submission`（`Submission`、`Command`、`ThreadSettings`）和 `event`（`Event`、`EventMsg`）位于 crate 根部，只依赖 `common`、`tool`，不得依赖 `model`。`adapter` 依赖 `event`，提供通用类别、过滤和 `EventAdapter` 转换契约。外部协议线类型和映射各自放在独立 crate：`tapestry-adapter-agui`、`tapestry-adapter-a2a`、`tapestry-adapter-cli`（命令行输出）；不得让协议 crate 依赖这些 adapter。回合内部消息 `TurnStep`、`ModelOutputEffect` 属于 `tapestry-core::turn`，不是协议类型。数据契约和 serde 行为放在协议层；`tapestry-core` 只保留行为（trait、状态机、IO、策略、渲染）。协议和 adapter crate 均不得依赖 `tapestry-core`（测试 dev-dependency 除外）。
- `tapestry-core` 负责运行时行为。`TapestryThread::spawn`（或 `TapestryThread::builder`，可注入 `Clock`、`EventStore`，并支持 `resume`）会创建有界的 Tokio 提交通道以及长期运行的工作任务。事件日志是唯一事实来源：所有事件经 `Recorder` 追加到 `EventStore`（默认 `InMemoryEventStore`），history 是事件日志的 `HistoryProjection` 投影，不单独保存。`ThreadHandle` 用于提交操作，并通过 `subscribe(after_log_position)` 获取 `EventSubscription` 消费事件。
- 线程工作任务保存会话配置，并按顺序处理提交。执行 `Command::StartTurn` 前，线程必须先收到 `Command::UpdateSettings`（唯一的配置入口）。运行时错误统一在线程工作任务边界转换为与原始提交关联的 `EventMsg::Error`。
- `AgentLoop::run_turn` 执行“模型 -> 可选工具 -> 模型”的迭代流程。它会发送生命周期事件、累计每次模型调用的 token 数，并在模型不再返回工具调用或超过 `max_tool_iterations` 时停止。
- `ModelClient` 和 `Tool` 是异步、满足 `Send + Sync` 的扩展点。`AgentLoop` 只调用 `ModelClient::stream`（文本增量 + 一个最终 `Completed`），其默认实现包装 `complete`，因此只支持一次性调用的客户端只需实现 `complete`。二者均以 `Arc<dyn ...>` 保存。模型请求携带请求开始时从事件存储读取的完整有序 `events` 快照；工具名称必须与 `ToolRegistry` 键名一致。
- `MockModelClient` 和 `EchoTool` 为本地开发及测试提供确定性行为。用户消息中的 `tool:echo` 等标记可以触发“模型/工具/模型”路径。
- Workspace adapters convert the ordered event log into AG-UI 1.0, A2A 1.0 or command-line outputs (`CliEventAdapter` renders only `ToolCallBegin`, `ToolCallEnd` and text `AssistantMessage` as `tool call:`/`tool result:` lines and unprefixed assistant text; `include_deltas` adds raw delta text); one adapter instance handles one thread. Use `EventFilter` to opt into AG-UI `CUSTOM` payloads and call `EventAdapter::finish` when the event stream ends.

`docs/architecture.md` 是架构总览及 Codex 与 Tapestry 模块映射的权威文档。其中提到的 app server 和 sandbox crate 尚处于规划阶段。`tapestry-cli` 是单轮命令行客户端，通过提交接口运行 agent（`-a` 可选，省略时使用内置系统提示词 `You are a helpful assistant.`），并将订阅到的事件经 `CliEventAdapter` 渲染后输出为文本行（`tool call: name(args)`、`tool result: name: output`、助手消息原文；失败信息写到 stderr）。

## 协议与事件约定

- `SubmissionId` 用于将每个事件与触发它的请求关联；`TurnId` 标识逻辑回合；`ToolCallId` 配对工具调用的开始和结束事件；它是模型提供方给出的不透明字符串（仅在提供方未给出时用 `ToolCallId::generate()` 兜底），任何逻辑都不得解析它；`MessageId` 由 core 为每次模型输出生成，关联同一条助手消息的 `AssistantMessageStart`、`AssistantMessageDelta` 和 `AssistantMessage`。新增操作或事件时必须保持这些 ID 的职责分离。
- 协议枚举使用内部标签的 serde 表示：`#[serde(tag = "type", rename_all = "snake_case")]`。可选的请求和设置字段使用 serde 默认值，并在值为 `None` 时省略。修改公开协议时应保持线格式兼容，并增加 serde 往返测试。
- 每个 `EventMsg` 必须在 `EventMsg::category()` 中归入唯一 `EventCategory`；新增事件时同步更新类别过滤测试和每个外部协议 adapter 的映射/测试（AG-UI 1.0、A2A 1.0、CLI）。
- 新增 UUID 类型的 ID 时使用 `common/ids.rs` 中私有的 `id_type!` 模式，使其保持为透明 UUID newtype，并具备一致的创建、解析、显示、哈希和 serde 行为；`ToolCallId` 是字符串 newtype，不使用该宏。
- `Event` 信封包含 `id`（`SubmissionId`）、`log_position`（由 store 分配，每个线程从 1 开始且连续）、`thread_id`、可选的 `turn_id`（只放在信封上，载荷中不重复）和 `timestamp_ms`。`HistoryItem` 除 `User`、`Assistant`、`ToolResult` 外，还有结构化条目 `EnvironmentContext`（每条为完整快照，最后一条是时间上下文基线）和 `SystemReminder`（保留在完整投影中；默认 formatter 仅发送最后一个用户、助手或工具结果之后的提醒，环境上下文不使提醒过期）。未完成回合的修复（为缺少结果的工具调用补 `failed` 结果）只在投影中进行，合成结果位于有效尾部提醒之前，且不发补偿事件。模型请求使用从事件日志读取的不可变事件快照，发给模型的文本统一由 formatter/prompt 渲染，不要在条目里存渲染后的字符串。工具结果统一为 `ToolResult`，token 用量统一为 `TokenUsage`。
- 新命令（`Command`）的分发逻辑放在 `thread::process_submission`；回合、模型和工具执行逻辑放在 `AgentLoop`。所有事件都通过 `Recorder::record` 记录（先追加，成功后再投影）；追加失败为 `CoreError::EventStore`，会结束当前提交；提交通道关闭映射为 `CoreError::ChannelClosed`。
- `ThreadSettings` 是局部更新：`ThreadConfig::apply_settings` 只更新值为 `Some` 的字段。第一次 `UpdateSettings` 会完成线程配置并发送 `SessionConfigured`；后续设置更新目前不会发送确认事件。
- `EventSubscription::collect_events_for` 会持续读取事件日志，直到收到匹配提交的终止事件（`TurnComplete`、`TurnAborted`、`Error` 或 `SessionConfigured`）。返回结果可能包含其他提交的中间事件，因此消费者必须使用 `Event.id` 进行关联。
- 线程工作任务会在提交循环内等待 `AgentLoop::run_turn` 完成，因此操作是串行执行的。如果尚未修改取消机制，不要假设排队的 `Interrupt` 能与正在执行的回合并发运行。

## 代码与测试约定

- 编写或修改代码时，必须同时编写覆盖对应行为的测试用例。新增功能应覆盖主要成功路径及关键边界情况；修复缺陷时必须添加能够复现该问题的回归测试。不得仅修改实现而不提供对应测试。
- 共享依赖版本统一声明在根目录的 `[workspace.dependencies]` 中；成员 crate 使用 `{ workspace = true }` 引用。保持 `tapestry-core` -> `tapestry-protocol` 的单向依赖关系。
- 各 crate 的公开 API 通过其 `lib.rs` 重新导出。新增需要公开的类型时同步更新 re-export。
- 领域错误使用 `thiserror` 枚举（`ProtocolError`、`CoreError`），并通过 `Result` 传播。模型或工具实现不应自行转换协议错误事件；该转换应保留在线程边界。
- 测试放在源码文件内的 `#[cfg(test)] mod tests` 模块中。异步测试使用 `#[tokio::test]`；协议测试验证序列化结构或往返相等性；核心测试使用 `MockModelClient`、内存通道和 `ToolRegistry`。
- 事件顺序属于可观察行为：成功回合以 `TurnStarted` 开始，每条助手消息依次为 `AssistantMessageStart`、零或多个 `AssistantMessageDelta`、`AssistantMessage`（文本以最终响应为准，增量不进入 history；空输出不发这些事件），每次工具调用由匹配的 `ToolCallBegin`/`ToolCallEnd` 包围，最后发送汇总的 `TokenCount` 和 `TurnComplete`。修改生命周期语义或公开操作/事件分类时，应同步更新测试和 `docs/architecture.md`。
