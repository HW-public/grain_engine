# Contributing to grain_engine

感谢你对 grain_engine 的兴趣！本文件说明如何搭建开发环境、构建与验证、以及提交贡献的规范。

## 项目定位（先读两分钟）

grain_engine 是一个 **Flutter 原生 WASM 插件引擎**：插件可用任意语言编写（当前仅提供 Rust PDK），编译为 `.wasm` 后在 VM 级沙箱中运行。当前处于 **ABI v0** 阶段，`docs/abi.md` 是唯一权威协议文档，v1 之前 ABI 可能调整——涉及 ABI 的改动请先开 issue 讨论（见下文 PR 要求）。

## Development Environment / 开发环境

需要两条工具链同时可用：

1. **Rust（stable）+ `wasm32-unknown-unknown` target**

   ```bash
   rustup toolchain install stable
   rustup target add wasm32-unknown-unknown
   ```

2. **Flutter（stable，含 Dart 3.8+）**

   ```bash
   flutter --version   # 要求 Dart >= 3.8.0
   ```

### 本机常见坑

- **Windows 无 MSVC Build Tools**：宿主侧构件（proc-macro 等）需要链接器，`.cargo/config.toml` 已配置使用 rustc 自带的 `rust-lld`，无需安装 VS Build Tools。wasm32 目标不受影响（rust-lld 本就是其默认链接器）。
- **首次使用 wasm_run**：其原生库不在 pub 包内，需运行 `dart run wasm_run:setup` 从 GitHub Releases 拉取（约 20MB），网络受限时可能需重试。
- **网络镜像**：`pdk/rust/.cargo/config.toml` 配置了 rsproxy 镜像（贡献者所在网络环境受限）。如果你的网络可直连 crates.io，可以忽略或自行调整。

## Build, Test, Verify / 构建、测试、验证

三步走（在仓库根目录依次执行）：

```bash
# 1. 构建示例插件（Rust PDK → wasm）
cd pdk/rust && cargo build --target wasm32-unknown-unknown --release

# 2. Rust 单元测试（10 项）
cargo test

# 3. Dart/Flutter 静态检查
cd ../.. && flutter pub get && flutter analyze
cd example/harness && flutter pub get && flutter analyze
```

### 六项判决（端到端验证）

harness 会真实装载 wasm 插件并访问真实网络，输出六项验证数据（与 `docs/abi.md` 的性能表对应）：

```bash
cd example/harness
WASM_DIR=../../pdk/rust/target/wasm32-unknown-unknown/release \
  flutter run -d <device> --release
```

Windows 桌面 runner 不透传命令行参数，插件目录必须通过 `WASM_DIR` 环境变量指定。

## Commit 规范

- **简短主题行**：一行说清楚做了什么，不要正文清单（细节写进文档或 PR 描述）。
- 推荐使用约定式前缀：`feat:` / `fix:` / `docs:` / `test:` / `chore:` / `refactor:`。
- 示例：`feat: 增加 grain_now() 时钟 import`、`fix: event_wait buf_cap 翻倍重试逻辑`。

## Pull Request 要求

- [ ] **CI 全绿**：`dart.yaml`（flutter analyze + dart format）与 `rust.yaml`（fmt/clippy/test/wasm 构建）两个 workflow 必须通过。
- [ ] **ABI 影响说明**：若改动影响 `docs/abi.md` 描述的协议（导出函数、import、字符串 ABI、metadata 格式），**必须先开 issue 讨论并达成一致**，再在 PR 中引用该 issue。ABI v0 处于冻结期：为存量插件的兼容性，任何 ABI 变更都需要充分的理由和迁移方案。
- [ ] **文档同步**：改了协议/行为就同步更新 `docs/`（权威文档）与 README 的相关段落。
- [ ] **验证证据**：附上 `cargo test` 输出、analyze 结果；行为性改动建议附 harness 六项判决数据（或说明为何不需要）。

## 行为准则（Code of Conduct）

请保持友善、尊重与建设性。正式的行为准则文档（CODE_OF_CONDUCT.md）后续补充；在此之前，以「对事不对人、假设对方善意」为基本准则。

## 提问与讨论

- Bug 与功能请求：用 GitHub Issue 模板。
- 一般性问题、设计探讨：用 GitHub Discussions。
