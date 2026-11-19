# Changelog

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 格式，
版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [0.1.0] - 2026-11-19

### Added

- 引擎宿主（`lib/`）：wasm_run FFI → wasmtime 装载/实例化，fuel 掐死、trap 隔离、
  多实例并存
- ABI v0（`docs/abi.md` 权威）：NUL 结尾 UTF-8 字符串 ABI、guest handle slab、
  `grain_*` 能力 import 白名单（HTTP / XPath / MD5 / Base64 / log / event_wait）
- Rust PDK（`pdk/rust/`）：`#[plugin_fn]` / `plugin_meta!` 宏，async/await 编译为
  状态机经宿主泵驱动；mock 宿主工具（`tools/mock_host.mjs`）
- 示例插件与可运行 harness（`example/harness`）：六项判决端到端验证
  （真实网络实测：fetch / 异步桥 / fuel / panic 隔离 / 内存曲线 / WBI 签名）
- 中英双语 README 与开发文档（`docs/abi.md`、`docs/plugin-development.md`）

### Notes

- ABI v0 为冻结期协议，v1 前可能调整；涉及 ABI 的改动请先开 issue 讨论
- 运行时依赖：wasm_run（MIT）→ wasmtime（Apache-2.0）
