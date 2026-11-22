# Tasks

## 1. Middleware trait

- [x] 1.1 新建 `crates/tapestry-core/src/middleware/mod.rs` 和 `middleware_trait.rs`，定义 `Middleware` trait（六个钩子，均有空操作默认实现）、`HookCtx`、`ToolDecision`，并在 `lib.rs` 中声明模块、重新导出这三个类型；验证 `cargo check -p tapestry-core` 通过，`lib.rs` 的导出测试能引用 `tapestry_core::Middleware`
- [x] 1.2 在 `middleware_trait.rs` 的 tests 中验证：只实现一个钩子的中间件，其余钩子保持空操作（`before_tool` 返回 `Continue`，请求、响应和结果不被修改）；验证 `cargo test -p tapestry-core middleware` 通过

## 2. AgentLoop 集成

- [x] 2.1 `AgentLoop` 新增 `middlewares` 字段（默认空）以及链式方法 `with_middleware`、`with_middlewares`；`new`/`with_clock` 签名不变；验证现有 `agent_loop` 测试全部通过，包括无中间件时事件序列不变的回归测试
- [x] 2.2 在 `forward` 中接入 `before_turn`（`TurnStarted` 之后）和 `after_turn`（`TokenCount` 之前），前置按正序、后置按逆序；用 `MockModelClient` + `tool:echo` 加记录型中间件编写测试，断言回合钩子包围其他钩子，且 `after_turn` 执行时 `TurnComplete` 尚未记录
- [x] 2.3 接入 `before_model`（组装请求后、`stream` 前）和 `after_model`（拿到最终响应后、`on_model_output` 前）；测试改写 system prompt 后模型收到新请求且无新增事件，以及把 `hello` 改为 `HELLO` 后 `AssistantMessage`、history 和 `last_agent_message` 都随之变化
- [x] 2.4 在 `run_tool` 中接入 `before_tool`（`ToolCallBegin` 前，执行后恢复 id 和 name，遇到 `Skip` 时停止后续 `before_tool`）和 `after_tool`（`ToolCallEnd` 前，逆序，结束后回写 id 和 name）；测试以下情况：echo 参数改写为 `patched`、改动 id 和 name 无效、`Skip` 时不执行工具且 Begin/End 成对、`after_tool` 改写为 `rewritten` 后进入下一次请求的 history、跳过时 `after_tool` 仍被调用、未知工具时也调用钩子
- [x] 2.5 多中间件测试：注册 A、B，断言每个节点前置钩子按 A→B、后置钩子按 B→A 调用；A 把参数改为 `a`、B 追加 `b` 后输出为 `ab`
- [x] 2.6 错误与状态测试：`before_model` 报错时模型不被调用、回合没有 `TurnComplete`、不调用 `after_turn`；在 `thread.rs` 中通过 `ThreadBuilder::agent` 注入带中间件的 `AgentLoop`，断言出现与提交关联的 `Error` 事件；再验证一个计数中间件在同一线程两个 echo 回合后计数为 2，且日志中没有额外事件

## 3. 文档与整体验证

- [x] 3.1 更新 `docs/architecture.md`（中间件节点与事件的相对位置、执行顺序、不记录事件、状态归属）以及 `AGENTS.md` 中的架构要点；验证文档中的节点位置与 design.md 一致
- [x] 3.2 运行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`，全部通过

## Workflow follow-up

- 评审通过后归档该 change，并确认 `openspec/specs/agent-middleware/spec.md` 已生成。
