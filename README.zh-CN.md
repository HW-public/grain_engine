# grain_engine

[English](README.md) | **简体中文**

**Flutter 原生的 WASM 插件引擎** —— 用任意语言编写插件，在 VM 级沙箱中运行。

[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)
[![Engine](https://img.shields.io/badge/engine-wasmtime%20(FFI)-blue.svg)](#)
[![Platforms](https://img.shields.io/badge/platforms-Android%20%C2%B7%20Windows%20%C2%B7%20macOS%20%C2%B7%20Linux%20%C2%B7%20Web-orange.svg)](#)

插件不再绑定单一脚本语言：用 **Rust / Go / AssemblyScript / C / C# / Zig** 等
任何能编译到 WebAssembly 的语言编写，产物是 `.wasm` 二进制 + 内嵌元数据，
装载即读、零执行。宿主（你的 Flutter App）经 FFI 嵌入 wasmtime 运行插件，
沙箱隔离、能力白名单、燃料掐断全部由 VM 保证。

## 特性

- **任意语言写插件** — 统一 ABI + PDK 吸收语言差异，插件作者不受语言绑定
- **VM 级沙箱** — 线性内存隔离；插件仅能调用宿主显式暴露的 `grain_*` 能力
  （HTTP / XPath / 加密 / 日志），无文件系统、无原生网络
- **燃料掐断** — 显式开启后死循环插件 0~5ms 内被 trap；支持单次调用上限
  （`perCallFuel` 每次调用前充值）与实例累计预算两种语义
- **故障隔离** — 插件 panic 只 trap 自身，同进程其他插件照常运行
- **元数据内嵌** — 名称/版本/类型/包名存于 `.wasm` custom section，
  不执行任何代码即可读取与索引
- **轻量** — 单插件内存约 2.3MB，冷启动毫秒级

## 30 秒上手

**写插件**（Rust，PDK 把 async/ABI 细节全部藏起来）：

```rust
use grain_pdk::{plugin_fn, plugin_meta, Error, Result, Value};

plugin_meta! {
    "name": "示例源",
    "version": "1.0.0",
    "type": "bangumi",
    "package": "example.com",
}

#[plugin_fn]
pub async fn latest(page: u32) -> Result<Value> {
    let resp = grain_pdk::http(
        "GET",
        &format!("https://example.com/latest?page={page}"),
        "{}", "",
    ).await?;
    // 解析 resp.body → 条目列表（serde_json 已在 PDK 依赖里）
    Ok(resp.body.into())
}
```

```bash
cargo build --target wasm32-unknown-unknown --release
# → target/.../release/my_plugin.wasm   就是一个完整插件
```

**用插件**（Dart / Flutter 宿主）：

```dart
final plugin = await GrainPlugin.instantiate('my_plugin.wasm');
final meta   = plugin.metadata;              // 不执行代码，custom section 直读
final latest = await plugin.callLatest(1);   // JSON 字符串
final search = await plugin.callSearch('孤独摇滚');
plugin.dispose();
```

跑通完整验证（真实网络，六项判决）：

```bash
cd pdk/rust && cargo build --target wasm32-unknown-unknown --release
cd ../.. && cd example/harness && flutter pub get
WASM_DIR=../../pdk/rust/target/wasm32-unknown-unknown/release flutter run -d windows --release
```

## 工作原理

```
┌────────────────────────────────────────────────┐
│ 你的 Flutter App                                │
│  ┌──────────────────────────────────────────┐  │
│  │ grain_engine（本仓库 lib/，~550 行 Dart） │  │
│  │  · 装载/实例化（wasm_run FFI → wasmtime） │  │
│  │  · grain_* 能力进口（dio/xpath/crypto）   │  │
│  │  · 异步泵 + NUL 字符串 ABI + fuel 控制    │  │
│  └──────────────────────────────────────────┘  │
├──────────── ABI v0（docs/abi.md 权威）─────────┤
│   plugin.wasm（任意语言编译， grain.metadata）   │
│   · 只能 import "grain"：白名单内 6 个能力      │
│   · async fn 由 PDK 宏转状态机，事件驱动        │
└────────────────────────────────────────────────┘
```

与 JS 插件引擎的本质区别：插件是**编译后的二进制**而非脚本文本——
审核者可反编译查看、运行时不含解释器攻击面、性能近原生。

## 实测数据（六项验证，真实网络；复现见 example/harness）

| # | 验证项 | 结果 |
|---|--------|------|
| 1 | 真实源拉取 | latest 969ms / 20 条（JS 引擎基线 491ms/17 条，同量级） |
| 2 | 异步桥 | search 436ms 精确命中；host 能力异步结果经 event_wait 直写 |
| 3 | fuel 掐断 | 死循环 0~5ms 内 trap（100k fuel 精确耗尽） |
| 4 | panic 隔离 | trap 被宿主捕获，同进程新实例照常装载 |
| 5 | 内存曲线 | ~2.3MB/插件（20 实例 RSS 118.6MB） |
| 6 | 复杂校验 | B 站 WBI 签名（nav→置换派生→MD5）全链路演通，实测 176ms/20 条 |

## 与同类方案

| | grain_engine | JS 引擎插件 | 声明式规则（Kazumi 类） |
|---|---|---|---|
| 插件语言 | 任意（WASM） | 仅 JS | 无代码（配置） |
| 表达力上限 | 图灵完备 + 沙箱内自实现算法 | 同左 | 低（表达不了签名/计算） |
| 安全 | VM 硬隔离 + 燃料 | 软约束沙箱 | 数据不可执行（最稳） |
| 生态起点 | 开荒 | 成熟 | 成熟 |
| 运行时体积 | wasmtime ~11MB | 引擎 ~1MB | 无 |

取舍说明：选 WASM 是用「生态从零」换「表达力 × 安全性 × 多语言作者群」。
引擎仓库的存在本身即生态冷启动的基础设施。

## 仓库结构

```
lib/            Dart 引擎包（宿主侧全部能力）
pdk/rust/       Rust PDK + 过程宏 + 示例插件 + 工具链
docs/           abi.md（协议权威）· plugin-development.md（编写指南）
example/        可运行 harness（六项判决一键复现）
```

## Roadmap

- [ ] PDK：AssemblyScript（TS 语法）· Go/TinyGo
- [ ] 宿主能力扩充：WebView 逃生舱（验证码/浏览器指纹场景）、WebSocket
- [ ] `grain_now()` 时钟进口（省一次 HTTP Date 往返）
- [ ] 插件热更新与来源签名校验
- [ ] pub.dev 发布

## 协议与许可

- ABI 版本：v0（见 [docs/abi.md](docs/abi.md)，预 v1 前可能调整）
- 本仓库：MIT（© 2026 HW）
- 运行时依赖：[wasm_run](https://pub.dev/packages/wasm_run)（MIT）→
  [wasmtime](https://wasmtime.dev/)（Apache-2.0），均为成熟开源件
