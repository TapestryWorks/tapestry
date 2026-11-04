# Proposal

## Why

模型没有时钟，无法回答“今天几号”，也无法理解“昨天”“下周”等相对时间。Codex 的做法是把当前日期作为 `<environment_context>` 片段追加进 history，并仅在内容变化时增量重发；不给每条消息打时间戳。Tapestry 需要同样的能力，且应在 history 尚未持久化的现在确定其语义，避免日后加载/压缩时返工。

## What Changes

- 新增时间上下文注入：每个回合开始（用户消息写入前），若“日期 / 时区”相对上次已发送的快照发生变化，则向 history 追加一条 `User`（无名称）消息，渲染为 `<environment_context>` 片段，仅包含变化的字段；无变化则不发。
- 新增 `Clock` 抽象（`Arc<dyn Clock>` 注入，含系统时钟与测试用固定时钟），时区获取方式须跨平台（Linux / macOS / Windows，且不破坏 wasm32 检查所约束的 `tapestry-protocol`）。
- 新增 `ContextSnapshot { date, timezone }`，由线程会话持有，用于 diff。
- `ThreadSettings` / `ThreadConfig` 新增可选开关 `include_time_context`，**默认开启**。
- 读时钟失败时降级：不使回合失败，改为在 history 追加 `<current_date status="unavailable"/>` 提示（同一状态不重复发送）。
- 约定：任何改写 history 前缀的操作（未来的压缩、回滚、从存储恢复）必须同时清空快照，使下一回合完整重发；通过会话上的单一入口强制执行。
- 不修改 `HistoryItem` 枚举，不为 history 条目添加时间戳字段；未来持久化时，时间戳只放在存储信封层，快照作为独立条目保存（仅在 design 中记录，本次不实现存储）。
- 不实现 Codex 的 `current_time_reminder`（实验特性，发行版默认关闭）。

## Capabilities

### New Capabilities
- `time-context`: 向模型提供当前日期/时区上下文的行为——何时注入、如何 diff、如何渲染、关闭与时钟失败时的行为、history 改写后的重发规则。

### Modified Capabilities
- `thread-events`: `ThreadSettings` 新增可选字段 `include_time_context`，需保持序列化省略与向后兼容的反序列化。

## Impact

- 代码：`tapestry-core`（新增 `clock`、`context` 模块；`thread.rs` 的 `Session` 增加快照；`agent.rs::run_turn` 在写入用户消息前注入）；`tapestry-protocol`（`ThreadSettings` 字段，保持不依赖 core）。
- 依赖：可能新增时间/时区 crate，须声明于 `[workspace.dependencies]` 并满足跨平台依赖约束；`tapestry-protocol` 不引入平台专有依赖。
- 文档：更新 `docs/architecture.md` 的 history 与回合生命周期描述。
- 行为：history 中会多出上下文类 `User` 条目；模型请求内容因此改变（默认开启），事件顺序契约不变，不新增事件。
