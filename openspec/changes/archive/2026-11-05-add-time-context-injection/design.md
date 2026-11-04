# Design

## Context

- `Session`（`thread.rs`）持有内存 `history: Vec<HistoryItem>`，目前没有压缩/回滚/持久化。
- `AgentLoop::run_turn` 先 `history.push(user)` 再进入 `drive`；现有 `reminder` 只推进每次请求的克隆，不入 history，与本变更需要的“持久入 history”语义不同。
- `tapestry-core` 与 `tapestry-protocol` 受 `cross-platform-build` 约束：依赖须能在三大桌面平台编译，protocol 须能在 wasm32 编译。
- 参考 Codex：日期作为 `<environment_context>` 片段进入 history，与上次快照 diff，仅变化时发送；时间戳只在存储信封层，不在消息上。详见 proposal。

## Goals / Non-Goals

**Goals:**
- 以最小改动让模型获得日期/时区，且 history 前缀稳定（利于缓存）。
- 时钟可注入，测试确定。
- 为未来持久化/压缩/回滚留出一致的“失效快照”约定。

**Non-Goals:**
- 不实现持久化、压缩、回滚本身。
- 不做每个 step 的检测（只在回合开始）；不做精确到秒的时间提醒。
- 不新增事件、不改 `HistoryItem` 枚举。

## Decisions

1. **作为普通 `User` 条目注入，不扩展 `HistoryItem`。** 
   备选：新增 `Context` 变体 / 给条目加时间戳。否决：会波及 `genai/mapping.rs`、所有 match 与序列化，且逐条时间戳破坏前缀稳定。

2. **检测时机为回合开始，用户消息之前。** 
   Tapestry 回合短，跨午夜的长回合不做增量。备选：每次 `CallModel` 前（Codex 的 per-step）；暂不需要，需要时可下沉而不改变 spec 的外部语义以外的部分。

3. **`ContextSnapshot` 状态机。** 会话持有 `context: ContextState`，取值：`Unsent`（需完整发送）、`Sent{date, timezone}`、`Unavailable`。
   - `Unsent` 或与当前不同 → diff 渲染，仅含变化字段（`Unsent` 时为全部）。
   - 时钟失败：`Unavailable` 若尚未处于该状态则追加 unavailable 条目并转入；已处于则不发。恢复后 `Unavailable → Sent` 按完整发送。
   - 开关关闭：不读时钟；若状态为 `Sent` 则追加 unavailable 条目转入 `Unavailable`；`Unsent` 保持不变（不发任何内容）。
   - 日期比较粒度为本地日历日字符串。

4. **`Clock` trait。** `trait Clock: Send + Sync { fn now(&self) -> Result<ClockReading, ClockError> }`，`ClockReading { local_date: String, timezone: String }`（或返回时间点由 core 格式化）。以 `Arc<dyn Clock>` 注入 `AgentLoop`/线程构造，默认 `SystemClock`，测试用 `FixedClock`/可编程失败的 clock。与 `ModelClient`/`Tool` 同风格。

5. **跨平台时间依赖。** 采用 `chrono`（`clock` 特性）获取本地日期与偏移，`iana-time-zone` 获取 IANA 名称；后者失败时回退 UTC 偏移（如 `+08:00`）。依赖声明在根 `[workspace.dependencies]`，仅用于 `tapestry-core`，不进入 `tapestry-protocol`。实现第一步须先验证 Linux/macOS/Windows（CI 矩阵）可编译，若 `tapestry-core` 将来需要 wasm 再单独评估。
   备选：`time` + `time-tz`、`jiff`。`jiff` 同样跨平台，若 `chrono` 的 `clock` 特性或依赖体积成为问题可替换；因 `Clock` 抽象存在，替换局限于一处。

6. **开关放在 `ThreadSettings.include_time_context: Option<bool>`，默认视为 `true`。** 协议层仅一个 `Option<bool>`，遵循 `skip_serializing_if` 与默认反序列化；`ThreadConfig::apply_settings` 只在 `Some` 时更新。

7. **history 改写的单一入口。** 在 `Session` 上提供 `replace_history(history)`（或等价方法），内部将 `ContextState` 置为 `Unsent`；未来压缩/回滚/恢复必须经此入口。当前无调用者，先通过单元测试保证行为。

8. **渲染格式。** 
   ```
   <environment_context>
     <current_date>2026-11-05</current_date>
     <timezone>Asia/Shanghai</timezone>
   </environment_context>
   ```
   unavailable 为 `<current_date status="unavailable" />`。仅输出变化的字段。用户条目 `name: None`，不受 `render_user_content` 前缀影响。

9. **失败回合的 history 修复。** 时间上下文条目在 `run_turn` 起始写入，先于用户消息；`repair_history` 只补缺失的工具结果，不受影响。条目一旦写入即视为“已发送”，即使回合随后失败也不回滚快照（模型请求若未发出，下一回合也不会重复，因为条目已在 history 中）。

### 未来兼容：持久化与加载（本次不实现）
- 存储信封带 `timestamp`（RFC3339），`HistoryItem` 不加字段；这对应 Codex 的 `RolloutLine { timestamp, item }`。
- `ContextState` 作为独立可序列化条目保存（体积很小，每次写完整状态，无需 merge patch）；加载时取最后一条作为基线，缺失则 `Unsent`。
- 压缩/回滚条目出现时基线重置；历史消息中的旧日期原样保留，不改写。
- 恢复到更晚的日期时，下一回合按普通 diff 自然追加新日期。
- 不得用信封 `timestamp` 反推基线。未知条目类型须跳过而非报错。

## Risks / Trade-offs

- [默认开启会改变所有模型请求] → 开关可关；spec 明确无变化不重发，成本为首回合一条短消息。
- [跨午夜的长回合模型日期过时] → 接受；回合短，下一回合即更新。需要时把检测下沉到每次模型调用前。
- [`iana-time-zone` 在个别环境失败] → 回退 UTC 偏移；再失败则按时钟失败处理（unavailable），不使回合失败。
- [时间依赖引入平台编译问题] → 先在 CI 三平台验证；依赖仅限 `tapestry-core`，不影响 protocol 的 wasm32 检查。
- [现有 `reminder` 路径与本机制并存导致混淆] → 在文档中区分：reminder 为临时请求内注入，时间上下文为持久 history 条目。
- [快照与 history 不同步（未经 `replace_history` 改写）] → 通过单一入口与回归测试约束。
