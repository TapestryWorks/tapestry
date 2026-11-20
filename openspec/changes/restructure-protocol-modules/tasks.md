# Tasks

## 1. 协议 crate 模块重组

- [x] 1.1 把 `ids.rs` 移到 `common/ids.rs`，把 `common.rs` 改为 `common/mod.rs` 并 `pub use ids::*`；更新各模块中的 `crate::ids` 引用；`cargo test -p tapestry-protocol` 通过，并新增测试，验证 `tapestry_protocol::common::TurnId` 等路径可用
- [x] 1.2 把 `client/event.rs` 移到 `src/event.rs`，把 `client/submission.rs` 移到 `src/submission.rs`，删除 `client/`；把 `ThreadSettings` 及其测试从 event 模块移到 submission 模块；`cargo test -p tapestry-protocol` 通过
- [x] 1.3 更新 `lib.rs` 的模块声明、重导出和顶部分层说明（`common` ← `tool` ← `history` ← `model`；`event`/`submission` 只依赖 `common`、`tool`）；`cargo check --workspace` 通过

## 2. react 移入 core

- [x] 2.1 把 `TurnStep`、`ModelOutputEffect` 及其往返测试从 `tapestry-protocol/src/react.rs` 移到 `tapestry-core` 的 turn 模块，删除 `react.rs` 及其重导出；`tapestry-core/lib.rs` 改为从本 crate 导出这两个类型；`cargo test -p tapestry-core turn::` 通过
- [x] 2.2 更新 `agent.rs`、`turn.rs` 的导入；`cargo check --workspace` 通过，并确认 `grep -rn TurnStep crates/tapestry-protocol` 没有结果

## 3. Op → Command，合并 Configure

- [x] 3.1 在 `submission.rs` 中把 `Op` 改名为 `Command`，`Submission.op` 改名为 `command`，删除 `Configure`，把 `ThreadSettings` 变体改名为 `UpdateSettings { settings }`；新增 serde 测试：三种命令往返、标签为 snake_case、序列化结果中有 `command` 字段而没有 `op` 字段、`configure`/`thread_settings` 无法解析；`cargo test -p tapestry-protocol` 通过
- [x] 3.2 修改 `thread::process_submission`：删除 `Configure` 分支，`UpdateSettings` 沿用原 `ThreadSettings` 的逻辑；`ThreadHandle::submit` 改为接收 `Command`
- [x] 3.3 迁移 `thread.rs` 中所有使用 `Op::Configure` 的测试，改用 `Command::UpdateSettings`；新增或确认测试覆盖 spec 场景：首次设置发出 `SessionConfigured`；后续设置只更新非空字段且不发事件；未配置时 `start_turn` 产生 `Error`；`cargo test -p tapestry-core thread::` 通过
- [x] 3.4 迁移 `examples/minimal_turn.rs`、`examples/genai_multi_provider.rs`；`cargo run -p tapestry-core --example minimal_turn` 正常运行完成

## 4. 文档

- [x] 4.1 更新 `docs/architecture.md`：模块映射表、分层图、`Op` 小节改为 `Command`、时序图中的 `Op::Configure` 改为 `UpdateSettings`、说明只有首次设置有确认事件；`grep -n "Op::\|Configure\|react" docs/architecture.md` 不再有过时引用
- [x] 4.2 更新 `AGENTS.md`（分层描述、`Op::Configure`/`Op::ThreadSettings`、`thread::process_submission` 说明）和 `README.md` 中的相关示例；`grep -rn "Op::" AGENTS.md README.md` 没有结果

## 5. 集成检查

- [x] 5.1 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace` 全部通过
- [x] 5.2 `openspec validate restructure-protocol-modules --strict` 通过
