# Design

## Context

现状（见 proposal.md - Why）：`AgentLoop::run_turn` 在单函数内完成决策、模型调用、工具执行与事件发送；`ModelRequest` 仅含 `user_message` 与 `tool_results`；工具 `execute` 返回 `Result`，未知工具和执行失败通过 `?` 中断回合；`thread::process_submission` 串行等待回合结束。约束：协议 crate 不得依赖 core；公开 API 通过 `lib.rs` 重导出；事件顺序是可观察行为；框架未上线，允许破坏性变更；核心与协议必须跨平台。

设计参考：Codex（`needs_follow_up` 终止条件、finish 语义）、Rig（`AgentRun` 无 IO 状态机、未知工具不崩溃）、AgentScope（只读 `next_action`、超限强制收尾、`ModelCallStart/End`、多 agent 视角投影）。

## Goals / Non-Goals

**Goals:**
- 决策与 IO 分离，使循环语义可用纯单元测试覆盖。
- history 成为循环的真实状态，并跨回合保留。
- 工具失败可被模型观察，终止语义用结构化的 `finish_reason` 表达。
- 以 CI 守住跨平台边界。

**Non-Goals:**
- 流式、回合内取消、并行工具、hook、审批、压缩、structured output。
- agent 间通信与连续 user 消息折叠。
- 为 history 设置上限或持久化。

## Decisions

### D1：`TurnMachine`（同步纯逻辑）+ `AgentLoop`（异步驱动）
状态机提供只读 `next_step()`，返回 `CallModel{iteration, allow_tools, reminder}`、`RunTools{calls}`、`Done{finish, last_agent_message}`；`on_model_output` 与 `on_tool_outcomes` 推进状态并修改 history。驱动器负责调用模型/工具、计时、发事件。`RunTools` 以批量形式表达，为后续并行执行预留。
- 备选：直接重写 `run_turn`。拒绝原因：事件顺序、终止、回灌规则会继续与 IO 缠在一起，测试需要异步 mock；三个参考框架都趋向拆分。

### D2：History 归会话状态
`HistoryItem` = `User{name: Option<String>, content}` | `Assistant{content: Option<String>, tool_calls}` | `ToolResult{call_id, tool_name, output, status}`。history 存放在线程 worker 的会话状态中，仅内存。回合开始写入用户消息；模型调用失败时，为尚无结果的工具调用补 `failed` 结果以保持合法。
- 备选：每回合只传当前消息。拒绝原因：无法多轮对话。

### D3：`name` 只属于 `User`，渲染在模型适配层
history 保持结构化；公共函数 `render_user_content(name, content)` 由 mock 与 genai 适配器共用，输出 `"{name}: {content}"`。不使用 provider 原生 `name` 字段（字符集与支持度不一）。助手侧无 `name`，自身身份通过 `agent_name` 在 system prompt 末尾追加一句声明。这与 AgentScope 的视角投影一致：assistant 即“自己”，他人（含其他 agent）为 user 侧发言。
- 备选：助手也带 `name`。拒绝原因：模型会模仿 `name:` 前缀输出。
- 备选：折叠为 `<history>` 块（AgentScope 做法）。暂缓：需要额外提示模板与各 provider 的 role 交替实测，作为后续适配器选项。
- 校验：`user_name` 必须非空且不含换行，违反时在线程边界返回错误且不改 history。内容中伪造 `bob: …` 的风险记为已知限制。

### D4：工具契约
`Tool::spec() -> ToolSpec{name, description, parameters: serde_json::Value}`；`execute` 返回 `ToolResult`（无 `Err`）。注册表提供 `specs()`。未知工具由驱动器直接生成 `not_found` 结果。选用 `serde_json::Value` 作 JSON Schema，不引入 `schemars`，保持依赖最小。

### D5：超限强制收尾
当迭代数达到 `max_tool_iterations` 且最近一次输出仍含工具调用：先把该次工具执行完并回灌，再产出一次 `CallModel{allow_tools:false, reminder:Some(..)}`，之后 `Done{ExceedMaxIters}`。reminder 仅附加到本次请求，不写入 history。收尾调用若仍返回工具调用，则忽略并发出 `Warning`。
- 备选：直接 `Error`（现状）。拒绝原因：丢失已完成的工作；AgentScope 的强制收尾体验更好。

### D6：协议改动保持增量，并接受必要的破坏
- 保留 `AgentMessage`，语义为“完整助手消息”；P2 流式再评估 Start/Delta/End。
- 新增 `ModelRequestStarted/Completed`、`Warning`；`ToolCallEnd` 增 `status`、`duration_ms`；`TurnComplete` 增 `last_agent_message`、`finish_reason`。
- `TurnAborted.reason` 改枚举 `TurnAbortReason`（目前仅 `interrupted`）并加可选 `message`。
- 新增 `Op::StartTurn.user_name`、`ThreadSettings.agent_name`，皆为可选且缺失时省略。
- 保留 `ToolCallEnd.success`，`status` 为权威状态。

### D7：`ModelClient` 保持非流式接口
`ModelRequest{system_prompt, history, tools, iteration}`；`ModelResponse` 含文本、工具调用（带 `ToolCallId`）、token 用量。流式 `ModelEvent` 留给 P2，当前接口不阻碍后续以默认实现聚合流。

### D8：跨平台守则
- `getrandom` 的 wasm 特性使用 `[target.'cfg(all(target_arch = "wasm32", target_os = "unknown"))'.dependencies]` 配置；其版本须与 `uuid` 解析到的版本一致，实施时用 `cargo tree` 确认。
- CI：ubuntu/macos/windows 做 `cargo check --workspace` 与 `-p tapestry-core --features genai`；单独任务对 `tapestry-protocol` 做 wasm32 检查；fmt/clippy/test 维持在 ubuntu。

## Risks / Trade-offs

- [history 无上限，长会话 token 膨胀] → 已知限制，压缩留到后续阶段，并在文档标注。
- [`Interrupt` 仍无法打断运行中回合] → 文档与 spec 明确，P3 引入取消。
- [genai 在 Windows/macOS 的构建未验证] → 由新 CI 矩阵暴露；失败时再评估其 TLS 后端。
- [wasm32 的 `getrandom` 版本不匹配] → 实施时 `cargo tree` 核对；本机未装该 target，需在 CI 或安装 target 后验证。
- [用户名前缀可被内容伪造] → 已接受，记为已知限制；基本校验仅拦截空值与换行。
- [破坏性变更面大（trait、协议、示例、测试）] → 项目未上线；按任务顺序先协议后核心，每步保持 `cargo test --workspace` 通过。
- [强制收尾 reminder 的措辞影响模型表现] → 作为常量集中维护，并在测试中仅断言“不入 history、不带工具”，不绑定具体措辞。

## Migration Plan

无线上数据迁移。按 protocol → core 类型 → 状态机 → 驱动器 → 适配器/示例 → CI/文档的顺序提交；每一步编译并通过测试。回滚为 git 回退。

## Open Questions

- 强制收尾 reminder 的最终措辞与语言（不影响 spec 与任务拆分）。
- P2 是否将 `AgentMessage` 替换为 Start/Delta/End 三段式。
