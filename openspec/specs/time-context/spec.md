# time-context Specification

## Purpose

向模型提供当前日期与时区，使其能理解“今天”“昨天”等相对时间。上下文以普通 history 条目的形式增量注入，仅在内容变化时重发，保持 history 前缀稳定。

## Requirements

### Requirement: 回合开始时注入时间上下文
当时间上下文开启时，线程 SHALL 在每个回合开始、写入用户消息之前，比较当前日期/时区与上次已发送的内容；若不同或从未发送，MUST 向 history 追加一条无名称的用户侧条目，且该条目位于本回合用户消息之前。

#### Scenario: 线程首个回合
- **WHEN** 开启时间上下文的线程执行第一个回合
- **THEN** history 依次为：时间上下文条目、用户消息，并包含当前日期与时区

#### Scenario: 同一天的后续回合
- **WHEN** 日期与时区均未变化，线程执行下一个回合
- **THEN** 不追加新的时间上下文条目

### Requirement: 仅在变化时增量发送
时间上下文 SHALL 仅包含相对上次已发送内容发生变化的字段；日期按本地日历日（`YYYY-MM-DD`）比较，同一日历日内的不同时刻 MUST NOT 触发重发。

#### Scenario: 跨日
- **WHEN** 两个回合之间本地日期由 `2026-11-04` 变为 `2026-11-05`，时区不变
- **THEN** 新条目仅包含 `current_date` 为 `2026-11-05`，不含 `timezone`

#### Scenario: 时区变化
- **WHEN** 本地时区变化而日期不变
- **THEN** 新条目仅包含新的 `timezone`

### Requirement: 时间上下文渲染格式
时间上下文条目 SHALL 渲染为 `<environment_context>` 包裹的片段，日期以 `<current_date>` 表示、时区以 `<timezone>` 表示；时区 SHALL 优先使用 IANA 名称，无法获取时回退为 UTC 偏移。

#### Scenario: 完整片段
- **WHEN** 首次发送日期 `2026-11-05`、时区 `Asia/Shanghai`
- **THEN** 条目文本包含 `<environment_context>`、`<current_date>2026-11-05</current_date>`、`<timezone>Asia/Shanghai</timezone>` 及对应结束标签

#### Scenario: IANA 名称不可用
- **WHEN** 系统无法提供 IANA 时区名称
- **THEN** `timezone` 使用 UTC 偏移（如 `+08:00`）

### Requirement: 默认开启且可关闭
时间上下文 SHALL 默认开启；客户端 MUST 能通过线程设置关闭。关闭时系统 MUST NOT 读取时钟或追加时间上下文条目。若此前已发送过日期而后被关闭，下一回合 SHALL 追加一条表示日期不可用的条目（`<current_date status="unavailable" />`），之后不再重复。

#### Scenario: 默认配置
- **WHEN** 线程未设置该开关
- **THEN** 回合按开启行为注入时间上下文

#### Scenario: 从未开启
- **WHEN** 线程在首个回合前即关闭该开关
- **THEN** history 中不出现任何时间上下文条目

#### Scenario: 中途关闭
- **WHEN** 已发送过日期的线程关闭该开关后执行回合
- **THEN** 追加一条日期不可用的条目；再下一个回合不再追加

### Requirement: 时钟读取失败不使回合失败
读取当前时间失败时，系统 SHALL 继续执行回合，并追加一条日期不可用的条目；同一不可用状态 MUST NOT 重复追加。时钟恢复后，下一回合 SHALL 重新发送完整的日期与时区。

#### Scenario: 时钟失败
- **WHEN** 回合开始时读取时钟失败
- **THEN** 回合正常执行，history 中追加日期不可用条目，不发出 `Error`

#### Scenario: 连续失败
- **WHEN** 日期已被标记为不可用，下一回合时钟仍失败
- **THEN** 不再追加条目

#### Scenario: 时钟恢复
- **WHEN** 日期被标记为不可用后，时钟恢复
- **THEN** 下一回合追加完整的 `current_date` 与 `timezone`

### Requirement: history 前缀被改写后完整重发
任何改写或截断 history 前缀的操作（包括未来的压缩、回滚、从存储恢复）MUST 同时使已发送的时间上下文失效，使下一回合重新发送完整的日期与时区。

#### Scenario: 替换 history
- **WHEN** 会话 history 被整体替换
- **THEN** 下一回合追加完整的 `current_date` 与 `timezone`，即使日期与此前相同

### Requirement: 时间上下文不改变事件与 history 条目类型
时间上下文条目 SHALL 作为普通用户侧条目存在，MUST NOT 新增 history 条目类型，MUST NOT 为 history 条目增加时间戳字段，也 MUST NOT 新增事件或改变既有事件顺序。

#### Scenario: 事件序列不变
- **WHEN** 回合注入了时间上下文条目
- **THEN** 事件序列仍以 `TurnStarted` 开始，并以汇总的 `TokenCount` 和 `TurnComplete` 结束，无额外事件类型
