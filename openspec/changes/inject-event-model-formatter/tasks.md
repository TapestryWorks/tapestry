# Tasks

## 1. 事件请求协议

- [x] 1.1 将 `ModelRequest.history` 替换为 `events: Vec<Event>`，保留其余字段和协议依赖方向，更新请求字段与 `HistoryItem::SystemReminder` 文档；用 `cargo test -p tapestry-protocol` 验证协议测试通过。
- [x] 1.2 添加包含用户、助手工具调用、工具结果、提醒及完整事件信封的请求 serde 往返测试，验证 `events` 存在、`history` 缺失、事件顺序和所有 ID 不变，同时保留响应往返测试。

## 2. formatter 共享历史准备

- [x] 2.1 修改 `event_to_history_item` 保留系统提醒，更新完整投影、实时 apply 和重放测试；验证提醒以结构化数据保留，流式增量仍不进入 history。
- [x] 2.2 在 `history_items_to_messages` 的列表级步骤筛选过期提醒，保持逐条转换仅做渲染；用表驱动测试验证讨论中的两个示例、无对话消息、空列表、用户/助手/工具结果边界、连续提醒及环境上下文穿插，并断言输入不被修改。
- [x] 2.3 复用并调整 formatter 中的工具结果补全，确保合成结果位于尾部有效提醒之前且不改变原有条目相对顺序；验证部分结果、截断、错误、中止、新回合、幂等性、不透明调用 ID，以及正常活跃回合不误补。
- [x] 2.4 为“缺失工具结果加尾部提醒”添加精确消息顺序回归测试，并验证事件格式化与 history 列表转换的提醒筛选一致；更新 formatter API 文档，明确投影视图和模型发送视图的区别。

## 3. 模型初始化与请求转换

- [x] 3.1 在 `GenaiModelClient` 中持有 `Arc<dyn Formatter>`，为 genai、OpenAI Chat、OpenAI Responses 增加 `with_formatter` 注入入口，默认构造使用默认策略；用构造与 clone 测试验证策略传递，确认既有公开路径可用。
- [x] 3.2 让流式与非流式调用使用统一的 formatter 请求构建路径，移除实际调用中的固定逐条映射旁路；用映射测试验证自定义消息、system prompt 和空/非空 tools，更新相关 API 文档及默认映射辅助入口说明。
- [x] 3.3 扩展本地 HTTP 测试，覆盖 Chat/Responses 各自流式和非流式的自定义 formatter 标记，验证实际请求载荷与原有端点，同时保留默认策略、响应解析、工具调用 ID 和用量行为断言。

## 4. 运行时及消费者迁移

- [x] 4.1 增加 Recorder 异步事件快照读取，AgentLoop 在请求开始事件和提醒事件记录后生成完整 `events` 请求，删除额外提醒追加；用捕获请求的测试验证跨回合完整性、日志顺序、快照不变性和提醒不重复。
- [x] 4.2 增加存储读取失败与提醒追加失败的回归测试，验证失败请求不调用模型、错误关联原始提交，且不回退为空事件或旧 history。
- [x] 4.3 迁移 mock 模型、自定义测试模型、trait 测试、示例及其余请求构造点，mock 复用共享历史准备而不依赖提供方消息类型；运行对应测试并验证 echo 工具往返仍然成功。
- [x] 4.4 更新强制收尾及线程恢复测试，验证旧提醒保留在日志/投影、新回合消息中过滤旧提醒，正常事件顺序、时间上下文和 token 统计不变。
- [x] 4.5 更新 README、`docs/architecture.md`、`AGENTS.md` 和相关公开示例，展示初始化注入与事件请求，移除“提醒永不进入投影/仅存在请求副本”的旧约定；验证示例编译且文档描述与规格一致。

## 5. 集成检查

- [x] 5.1 运行 `cargo fmt --all -- --check`、`cargo check --workspace`、`cargo clippy --workspace --all-targets -- -D warnings` 和 `cargo test --workspace`，验证整个工作区及外部事件 adapter 没有回归。
- [x] 5.2 运行 `cargo run -p tapestry-core --example minimal_turn`，验证默认模型路径完整执行；运行 `openspec validate inject-event-model-formatter --strict`，确认实现与规格及任务状态一致。

## Workflow follow-up

- 完成实现和评审后，按前序变更到本变更的顺序处理归档；本次 proposal 不自动归档或改写其他变更。
- 归档本变更前，检查前序新合入的事件投影要求，确保“提醒不进入投影”的旧条款被本变更的保留提醒规则同步替代，避免主规格残留冲突。
