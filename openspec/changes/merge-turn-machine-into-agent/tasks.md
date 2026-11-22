# Tasks

## 1. Agent 策略 trait 与 ReactAgent

- [x] 1.1 重写 `crates/tapestry-core/src/agent/agent_trait.rs`：定义同步的 `Agent: Send` trait（`next_step`、`on_model_output`、`on_tool_results`）、不含 `iteration` 和 serde 的 `TurnStep { CallModel, Done }`，以及 `AgentFactory` 类型别名；验证 `cargo check -p tapestry-core` 中该文件没有错误
- [x] 1.2 新建 `agent/react_agent.rs`，把 `turn.rs` 中的 `TurnMachine` 迁移为 `ReactAgent`（使用 `NeedModel / AwaitTools / Finished` 三个阶段，自行计数迭代，到达上限时返回 `allow_tools: false` 和 `ToolLimitReached`，在无工具调用的迭代中返回空的保留列表）；迁移原有纯同步单测，覆盖完成、工具循环、强制收尾和收尾时丢弃调用；验证 `cargo test -p tapestry-core react_agent` 通过
- [x] 1.3 删除 `turn.rs` 和 `ModelOutputEffect`；在 `agent/mod.rs` 和 `lib.rs` 中导出 `Agent`、`AgentFactory`、`ReactAgent`、`TurnStep`，并更新 `lib.rs` 兼容测试（`ReactAgent` 可以作为 `&dyn Agent` 使用）；验证 `cargo test -p tapestry-core compatibility_tests` 通过

## 2. AgentLoop 驱动器

- [x] 2.1 `AgentLoop` 新增 `agent_factory` 字段（默认创建 `ReactAgent`）和 `with_agent_factory`；删除 `impl Agent for AgentLoop`，`forward` 保留为固有方法；按 design D3 改写主循环（由驱动器计数迭代，删除 `RunTools` 分支，丢弃调用时由驱动器发出 `Warning`）；验证现有 `agent_loop` 测试在不改断言的情况下全部通过
- [x] 2.2 实现保序子序列校验、`allow_tools == false` 时禁止保留调用，以及 `max_tool_iterations + 1` 次模型调用的硬上限，违反时返回 `CoreError::Internal`；在 `agent_loop.rs` 中使用脚本化策略补充测试：伪造调用、重排调用、无工具时保留调用、无限循环（断言模型最多被调用 `max+1` 次且没有 `TurnComplete`），以及失败时未执行任何工具
- [x] 2.3 使用自定义策略补充行为测试：纯文本后强制再跑一轮（两组 `ModelRequestStarted`/`ModelRequestCompleted`，结束原因由策略给出）；只保留第二个调用（只执行第二个调用，助手消息只含该调用，并出现提及 1 个被忽略调用的 `Warning`）；策略收到的结果顺序和 ID 与调用一致；断言 spec 中自定义策略下的完整事件顺序
- [x] 2.4 每个回合使用独立策略实例的测试：工厂闭包统计创建次数，同一 `AgentLoop` 跑两个回合后创建次数为 2，且第二个回合的策略从初始状态开始；验证 `cargo test -p tapestry-core agent_loop` 通过

## 3. 线程装配

- [x] 3.1 把 `ThreadBuilder::agent(Arc<dyn Agent>)` 替换为 `agent_loop(AgentLoop)`，线程侧改为持有 `Arc<AgentLoop>`，`process_submission` 接收 `&AgentLoop`；更新 `thread.rs` 中两个使用中间件的测试；新增一个通过 `agent_loop` 注入自定义策略工厂的线程测试，断言策略生效，且出现与提交关联的 `TurnComplete`；验证 `cargo test -p tapestry-core thread` 通过
- [x] 3.2 检查 `tapestry-cli` 和 examples 是否引用了 `ThreadBuilder::agent`、`TurnMachine` 或 `Agent`，有引用就同步修改；验证 `cargo check --workspace` 通过

## 4. 文档与整体验证

- [x] 4.1 更新 `docs/architecture.md`：Codex 映射表、turn/agent 章节（`Agent` 策略与 `AgentLoop` 驱动器的职责、驱动器不变量）、时序图参与者（`TurnMachine` 改为 `Agent`）和测试章节，并写明 BREAKING 迁移方式；更新 `AGENTS.md` 中 `TurnStep`/`ModelOutputEffect` 所属位置和 `AgentLoop` 的描述；验证文档中已无 `TurnMachine`、`ModelOutputEffect`、`ThreadBuilder::agent(` 的残留（`grep` 为空）
- [x] 4.2 运行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`，全部通过；运行 `cargo run -p tapestry-core --example minimal_turn`，输出正常

## Workflow follow-up

- 评审通过后归档该 change，并确认 `openspec/specs/agent-turn-policy/spec.md` 已生成。
