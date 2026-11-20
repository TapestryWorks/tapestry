# Design

## Context

动机与范围见 `proposal.md`。当前 `Formatter` 已提供事件到 history、history 到 genai 消息的默认转换，但 `GenaiModelClient` 的请求映射直接调用逐条消息转换函数，没有使用 formatter 实例。OpenAI Chat 与 Responses 封装均委托给该客户端。

`Recorder` 持有事件存储和实时 history 投影，AgentLoop 用投影生成 `ModelRequest.history`，并额外把本次提醒加到副本。现有 `event_to_history_item` 过滤提醒；`HistoryProjection::fold` 还会修复截断回合。该结构需要调整，但事件日志仍是唯一事实来源，协议层不得依赖 core 或 genai。

本设计以前序 `event-sourced-thread-history`、`organize-model-and-formatter-modules` 的工作区结果为基础。主规格尚未合入前序的事件投影要求；本变更的增量补充当前主规格，并明确替代前序“提醒不进入投影”的语义。归档时先处理前序变更，再合入本变更，避免旧增量覆盖新规则。

## Goals / Non-Goals

**Goals:**
- 调用方提供内部事件，模型持有可替换 formatter；流式与非流式路径一致。
- 保持投影、提醒有效性、工具调用修复这三类规则边界清晰，并能独立测试。
- 既支持实时请求，也支持对同一事件快照重新转换，不依赖 formatter 的跨请求可变状态。

**Non-Goals:**
- 不增加提供方、不直接实现 OpenAI JSON 协议、不替换 genai。
- 不删除 `HistoryItem` 或运行时投影，不让 formatter 获取存储或订阅事件。
- 不改变事件信封、事件 adapter 输出、模型响应、token 统计或取消机制。
- 不引入分页上下文窗口、增量 formatter 缓存、服务端 Responses continuation 或历史请求字段的双输入兼容层。

## Decisions

### 1. 请求携带完整事件快照，存储读取留在运行时

`ModelRequest.history: Vec<HistoryItem>` 改为 `events: Vec<Event>`，保留其余字段。`model` 协议依赖 `event`，但 `event` 不反向依赖 `model`，维持协议分层。

Recorder 提供异步事件快照读取，使用其已有 store 和 thread ID 读取从日志起点开始的事件。AgentLoop 在写入 `ModelRequestStarted` 和可选 `SystemReminder` 后获取快照，再调用模型；后续追加不会改变已构造的值。当前线程串行处理提交，不需要增加长期事件副本或并发快照设施。读取失败按现有 `CoreError::EventStore` 传播至提交边界，不能回退为空事件或旧 history。

保留 Recorder 的 history 投影供时间上下文判断、恢复和检查使用，但它不再是模型请求字段。拒绝同时支持 `events` 和 `history`，避免双事实来源和优先级歧义。一次性克隆整个日志是此次明确接受的开销。

### 2. formatter 由实际执行请求的客户端持有

`GenaiModelClient` 持有 `Arc<dyn Formatter>`。现有 `from_model` / `from_config` 使用默认 `OpenAiFormatter`，增加链式 `with_formatter(Arc<dyn Formatter>)` 注入入口；OpenAI Chat 与 Responses 封装暴露同样入口并向内传递实例。这样保留既有默认构造调用，同时满足模型初始化时选择 formatter 的需求。clone 共享策略实例，因此 trait 保持 `Send + Sync`。

`complete` 和 `stream` 使用同一个内部请求构建入口：先调用 formatter 转换事件，再映射 system prompt 与 tools。现有公开的默认请求映射辅助函数可以保留为默认策略的便捷包装，但真实模型调用不能绕过注入实例。formatter 无需处理响应或网络 IO。

继续使用 `genai::chat::ChatMessage` 作为 formatter 输出；这里的 OpenAI formatter 是 genai 消息策略，而非最终 OpenAI 线格式编码器。协议 crate 中不出现此类型。

替代方案是每次请求传入 formatter，或在 AgentLoop 中先转换；前者让请求混入行为对象且难以序列化，后者使 AgentLoop 耦合提供方消息类型，因此均不采用。

### 3. 投影保留提醒，列表级转换负责有效范围

`event_to_history_item` 将 `SystemReminder` 转为 `HistoryItem::SystemReminder`。历史投影不因提醒过期而删除它，历史检查、恢复与重新 fold 保持一致。修改 `HistoryItem` 的“仅存在于请求副本”文档和对应断言。

提醒筛选位于 `history_items_to_messages` 的列表级准备步骤，逐条 `history_item_to_message` 只负责渲染一个已选中的条目。算法查找最后一个 `User`、`Assistant` 或 `ToolResult`，保留所有非提醒条目和边界之后的所有提醒；没有边界时保留全部提醒。不按 turn 或 iteration 过滤，也不把 `EnvironmentContext` 当作边界。

```text
R1, User, Assistant, R2, Context, R3
          |
          +-- 最后一个真实对话条目

User, Assistant, R2, Context, R3
```

提醒筛选只作用于发送视图，不修改输入 history 或日志。默认方法可复用；自定义 formatter 可以覆盖映射策略，不要求重写事件修复逻辑。

### 4. 补全由 formatter 中共享投影逻辑负责，保留提醒边界

继续复用 formatter 内的 `HistoryProjection` 和缺失工具结果补全辅助逻辑，不在各模型实现或各提供方 formatter 中复制补全算法。错误、回合中止、下一回合开始和离线 fold 的日志末尾仍触发已有修复规则；实时 `apply` 不因一次读取就关闭活跃回合。模型调用发生在真实工具结果记录后，因此正常循环不能产生额外失败结果。

调整共享修复中合成结果的插入位置：放在所属助手调用及已存在的关联工具结果之后、其后的尾部提醒之前，而不是无条件追加到提醒后面。原有条目之间的相对顺序不变。这样既保证调用结果顺序合法，也使列表级转换不会把合成结果误当作一个发生在提醒之后的新事实；无需给公开的 `HistoryItem` 增加来源字段。

```text
真实输入： User, Assistant(call X), Reminder
共享补全： User, Assistant(call X), FailedResult(X), Reminder
模型消息： User, Assistant(call X), FailedResult(X), Reminder
```

缺少哪些结果必须依据真实已有结果判断，只补缺失项，保留不透明调用 ID，重复转换不得重复生成结果。补全只存在于投影/发送视图，不记录补偿事件。测试同时覆盖直接 history 消息转换、事件格式化和实时投影与重新 fold 的一致性，防止两个入口语义分叉。

### 5. AgentLoop 只记录一次提醒，全部模型入口迁移

AgentLoop 保持当前可观察事件顺序，不再克隆 history 或手动追加 `HistoryItem::SystemReminder`。强制收尾时先记录提醒，再取事件快照；写入失败时不调用模型。后续真实用户、助手或工具结果会自然使旧提醒过期。

mock 模型改为读取请求事件并复用共享历史准备规则来定位用户消息和工具结果，避免保留旧请求结构或依赖 provider 消息类型。迁移自定义测试模型、Chat/Responses trait 测试、示例和所有请求构造点。公开新增入口通过既有模块与 crate-root 导出路径可用。

## Risks / Trade-offs

- [每次请求复制完整事件日志会增加内存和处理成本] → 本次优先保持无状态与可回放；不新增独立持久日志副本，后续优化不得改变完整快照语义。
- [提醒投影语义与旧规格冲突] → 明确记录前序关系，更新代码文档、测试和架构约定；归档按前序到本变更的顺序执行。
- [合成结果位置错误导致提醒丢失或工具消息不合法] → 增加未完成调用加尾部提醒、多调用部分完成、环境上下文穿插、重复转换的精确顺序测试。
- [外层保存了 formatter，但底层仍走固定映射] → 用本地 HTTP 测试观察实际请求标记，分别覆盖 Chat/Responses 和 complete/stream，而不只测试字段存在。
- [事件请求是破坏性 API/serde 变更] → 同步迁移工作区所有消费者并明确文档；不静默读取旧 `history` 为新请求输入。
- [全量 fold 与活跃投影修复时机不同] → 正常请求必须在真实工具结果记录后生成；保留截断快照的修复用例与活跃回合不误补用例。

## Migration Plan

1. 更新协议字段和 serde 测试，保留事件线格式不变。
2. 修改共享投影、补全和提醒筛选规则，并添加回归测试。
3. 接通模型 formatter 注入与统一请求映射，迁移运行时、mock、示例和自定义测试模型。
4. 更新直接相关文档，运行格式检查、workspace Clippy 和测试；本地 HTTP 用例不依赖外部凭证。
5. 本次只创建规划产物，不执行前序变更归档。实现后按前序关系归档；若需回滚实现，整体撤销此次请求字段及其消费者迁移，不单独回退 formatter 或协议字段。
