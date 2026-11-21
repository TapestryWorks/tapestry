# Tasks

## 1. 新建 crate

- [x] 1.1 创建 `crates/tapestry-adapter-cli`（`Cargo.toml` 使用 workspace 元数据，常规依赖为 `tapestry-protocol` 与 `serde_json`），并加入根 `Cargo.toml` 的 `members`；以 `cargo check -p tapestry-adapter-cli` 通过验证

## 2. 实现 CliEventAdapter

- [x] 2.1 在 `src/lib.rs` 中实现 `CliAdapterOptions { include_deltas }`（默认 `false`）与 `CliEventAdapter`（`new`、`with_options`、`Default`），定义 `CliOutput` 并以 `Display` 渲染，实现 `EventAdapter<Output = CliOutput>`：默认只为 `ToolCallBegin`、`ToolCallEnd`、有文本内容的 `AssistantMessage` 产生输出，`include_deltas` 时额外输出增量文本，`finish` 为空
- [x] 2.2 在同文件 `#[cfg(test)] mod tests` 中覆盖规格场景：流式回合只渲染助手原文、逐类型白名单、工具调用参数渲染（含空参数）、工具结果状态渲染、空白助手内容跳过、启用增量、`finish` 为空；以 `cargo test -p tapestry-adapter-cli` 通过验证

## 3. 接入 tapestry-cli

- [x] 3.1 在 `crates/tapestry-cli/Cargo.toml` 添加 `tapestry-adapter-cli` path 依赖；`run_turn` 创建一个 `CliEventAdapter` 传给 `print_events`，后者对已关联的事件调用 `adapt` 后写出结果，终止判定仍基于原始事件，流结束时写出 `finish` 结果；`Error`/`TurnAborted` 的错误消息带上事件中的消息/原因
- [x] 3.2 更新/新增 `tapestry-cli` 测试（模拟端点支持多次请求）：流式文本回合只输出 完整文本原文；工具调用回合依次输出 `tool call:`、`tool result:`、助手原文三行；模型错误时无输出行且错误包含错误消息；以 `cargo test -p tapestry-cli` 通过验证

## 4. 默认系统提示词

- [x] 4.1 `-a` 改为可选，省略时使用内置 `DEFAULT_SYSTEM_PROMPT`（`You are a helpful assistant.`），agent 名称仍在读取配置前校验；新增配置与端到端测试，并更新 README；以 `cargo test -p tapestry-cli` 通过验证

## 5. 文档

- [x] 5.1 更新 `docs/architecture.md`（crate 映射表新增 `tapestry-adapter-cli`，adapter 小节说明 CLI 只输出工具调用、工具结果和助手消息）与 `AGENTS.md` 中的 adapter crate 列表及“新增事件需同步 adapter”说明；人工检查文档与实现一致

## 6. 集成验证

- [x] 6.1 运行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace` 全部通过

## Workflow follow-up

- 评审通过后归档该变更，并确认 `openspec/specs/cli-adapter/spec.md` 生成正确。
