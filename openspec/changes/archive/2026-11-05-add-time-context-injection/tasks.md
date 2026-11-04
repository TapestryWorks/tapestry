# Tasks

## 1. 协议与配置

- [x] 1.1 在 `tapestry-protocol` 的 `ThreadSettings` 新增 `include_time_context: Option<bool>`（`serde(default, skip_serializing_if = "Option::is_none")`），并在 `ThreadConfig` 中新增默认 `true` 的同名字段，`apply_settings` 仅在 `Some` 时更新；验证：新增 serde 往返测试、旧 JSON 缺字段解析测试、局部更新测试通过（`cargo test -p tapestry-protocol`、`cargo test -p tapestry-core`）

## 2. 时钟抽象与依赖

- [x] 2.1 在根 `[workspace.dependencies]` 声明 `chrono`（`clock`）与 `iana-time-zone`，仅由 `tapestry-core` 引用；验证：`cargo check --workspace` 通过，`cargo tree -p tapestry-protocol` 不含这两个 crate
- [x] 2.2 新增 `tapestry-core::clock`：`Clock` trait、`SystemClock`（本地日期 `YYYY-MM-DD` + IANA 时区名，失败回退 UTC 偏移）、测试用可编程 `FixedClock`（可设置时间与失败），并在 `lib.rs` 重新导出；验证：单元测试覆盖日期格式、IANA 回退到偏移、FixedClock 失败路径
- [x] 2.3 核对跨平台：确认 CI 矩阵（Linux/macOS/Windows，含 `genai` 特性）可编译新依赖；验证：本地 `cargo check --workspace` 通过，并检查 `.github/workflows` 的构建矩阵覆盖这三个平台

## 3. 上下文状态与渲染

- [x] 3.1 新增 `tapestry-core::context`：`ContextState`（`Unsent` / `Sent{date, timezone}` / `Unavailable`，派生 `Serialize/Deserialize` 以便未来持久化）、diff 与渲染函数，输出 `<environment_context>` 片段及 `<current_date status="unavailable" />`；验证：单元测试覆盖首次完整发送、同日不重发、跨日仅日期、时区变化仅时区、不可用去重、恢复后完整发送、渲染文本精确匹配
- [x] 3.2 在 `Session` 中持有 `ContextState`，并新增 `replace_history` 单一入口（改写 history 的同时置为 `Unsent`）；验证：单元测试证明调用后下一回合重发完整上下文

## 4. 回合注入

- [x] 4.1 向 `AgentLoop`/线程构造注入 `Arc<dyn Clock>`（默认 `SystemClock`），在 `run_turn` 写入用户消息前按状态机追加上下文条目（`name: None`），关闭开关时不读时钟，时钟失败不产生 `Error`；验证：使用 `MockModelClient` + `FixedClock` 的测试覆盖首回合顺序（上下文在用户消息前）、同日不重发、次日增量、关闭从未发送/中途关闭、时钟失败与恢复、回合失败后 history 仍合法
- [x] 4.2 验证事件契约未变：测试断言注入后事件序列仍为 `TurnStarted` … `TokenCount`、`TurnComplete`，且 `genai` 映射对该条目按普通用户消息渲染（无名称前缀）；验证：`cargo test --workspace` 通过
- [x] 4.3 更新 `examples/minimal_turn` 以确认示例仍可运行；验证：`cargo run -p tapestry-core --example minimal_turn` 成功

## 5. 文档与质量门禁

- [x] 5.1 更新 `docs/architecture.md`：history 小节增加时间上下文、`Clock`、与临时 `reminder` 的区别，以及“改写 history 必须经 `replace_history`”和未来持久化约束；验证：文档与 `openspec/changes/add-time-context-injection/design.md` 一致，人工核对
- [x] 5.2 运行与 CI 一致的检查；验证：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace` 全部通过
