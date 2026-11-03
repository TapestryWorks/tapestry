# cross-platform-build Specification

## Purpose

约束 workspace 的依赖边界并通过 CI 持续验证，使核心与协议 crate 在主流桌面平台上都能编译，且协议 crate 保持足够轻量以支持 wasm32。

## Requirements

### Requirement: 核心与协议 crate 无平台专有依赖
`tapestry-protocol` 与 `tapestry-core` SHALL NOT 依赖仅在单一操作系统上可编译的 crate，也 MUST NOT 使用在构建时下载平台二进制的构建脚本。

#### Scenario: 依赖审查
- **WHEN** 向这两个 crate 添加新依赖
- **THEN** 该依赖必须能在 Linux、macOS、Windows 上编译，否则须隔离到独立 crate 或特性之后

### Requirement: 三大桌面平台构建检查
CI SHALL 在 Linux、macOS、Windows 上运行 workspace 构建检查，并对 `tapestry-core` 启用 `genai` 特性再检查一次。

#### Scenario: 任一平台编译失败
- **WHEN** 某平台的构建检查失败
- **THEN** CI 整体失败

### Requirement: 协议 crate 的 wasm32 检查
CI SHALL 对 `tapestry-protocol` 在 `wasm32-unknown-unknown` 目标上运行构建检查；该 crate 在此目标上 MUST 能编译，包括 ID 生成所需的随机源。

#### Scenario: wasm32 构建
- **WHEN** 针对 `wasm32-unknown-unknown` 构建 `tapestry-protocol`
- **THEN** 构建成功

### Requirement: 既有质量门禁保持
CI SHALL 继续运行格式检查、将警告视为错误的 Clippy 检查和整个 workspace 的测试。

#### Scenario: 存在 Clippy 警告
- **WHEN** 提交引入 Clippy 警告
- **THEN** CI 失败
