---
name: Bug report
about: 报告 grain_engine 的缺陷（引擎宿主、Rust PDK、ABI 协议）
title: "[bug] "
labels: bug
---

## 环境

- OS / 平台（Android / Windows / macOS / Linux / Web）：
- Flutter 版本（`flutter --version`）：
- Rust toolchain（`rustc --version`）：
- grain_engine 版本 / commit：

## 复现命令

请给出从干净目录出发的最小复现步骤（命令或最小代码）：

```bash
# 例如
cd pdk/rust && cargo build --target wasm32-unknown-unknown --release
cd example/harness && WASM_DIR=../../pdk/rust/target/wasm32-unknown-unknown/release flutter run -d windows --release
```

## 期望行为


## 实际行为

（如有错误输出 / 堆栈，请完整粘贴）

## 证据（二选一）

- [ ] 六项 harness 输出（`example/harness` 完整输出，含失败项）
- [ ] 最小可复现 `.wasm` 附件（或产生它的最小插件源码）

## 补充说明

（网络环境、是否使用镜像、是否首次运行 wasm_run:setup 等可能有帮助的信息）
