# grain_engine

[English](README.md) | **简体中文**

**A WASM plugin engine, native to Flutter** — write plugins in any language and run them in a VM-grade sandbox.

[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)
[![Engine](https://img.shields.io/badge/engine-wasmtime%20(FFI)-blue.svg)](#)
[![Platforms](https://img.shields.io/badge/platforms-Android%20%C2%B7%20Windows%20%C2%B7%20macOS%20%C2%B7%20Linux%20%C2%B7%20Web-orange.svg)](#)

Plugins are no longer bound to a single scripting language: write them in **Rust,
Go, AssemblyScript, C, C#, Zig** — any language that compiles to WebAssembly.
The artifact is a `.wasm` binary with embedded metadata, readable at load time
with zero execution. Your Flutter app embeds wasmtime over FFI to run plugins;
isolation, capability whitelisting and fuel-based termination are all enforced
by the VM itself.

## Features

- **Any-language plugins** — one ABI + per-language PDKs absorb the differences;
  plugin authors are not locked into a language
- **VM-grade sandbox** — linear-memory isolation; plugins can only call the
  `grain_*` capabilities the host explicitly exposes (HTTP / XPath / crypto /
  log). No filesystem, no native networking
- **Fuel termination** — opt-in; infinite loops trapped in 0~5ms, with both
  per-call (`perCallFuel`, recharged before each call) and cumulative-budget modes
- **Fault isolation** — a panicking plugin traps alone; other instances in the
  same process keep running
- **Embedded metadata** — name/version/type/package live in a `.wasm` custom
  section; index and inspect plugins without executing a single instruction
- **Lightweight** — ~2.3MB per plugin, millisecond-class cold start
- **Batteries included** — the wasm_run native library is embedded (Windows
  x64); the first `instantiate` places it into wasm_run's official lookup
  directory automatically — no setup command, no download. Other platforms:
  `dart run wasm_run:setup` (see [native/NOTICE.md](native/NOTICE.md))

## Quickstart

**Author a plugin** (Rust — the PDK hides all async/ABI details):

```rust
use grain_pdk::{plugin_fn, plugin_meta, Error, Result, Value};

plugin_meta! {
    "name": "Example Source",
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
    // Parse resp.body into an item list (serde_json is already a PDK dependency)
    Ok(resp.body.into())
}
```

```bash
cargo build --target wasm32-unknown-unknown --release
# → target/.../release/my_plugin.wasm   — that's a complete plugin
```

**Consume a plugin** (Dart / Flutter host):

```dart
final plugin = await GrainPlugin.instantiate('my_plugin.wasm');
final meta   = plugin.metadata;              // custom section, zero execution
final latest = await plugin.callLatest(1);   // JSON string
final search = await plugin.callSearch('Lycoris Recoil');
plugin.dispose();
```

Run the full verification suite (real network, six verdicts):

```bash
cd pdk/rust && cargo build --target wasm32-unknown-unknown --release
cd ../.. && cd example/harness && flutter pub get
WASM_DIR=../../pdk/rust/target/wasm32-unknown-unknown/release flutter run -d windows --release
```

## How it works

```
┌────────────────────────────────────────────────┐
│ Your Flutter App                                │
│  ┌──────────────────────────────────────────┐  │
│  │ grain_engine (lib/ in this repo,         │  │
│  │   ~550 lines of Dart)                    │  │
│  │  · load/instantiate (wasm_run FFI →      │  │
│  │    wasmtime)                             │  │
│  │  · grain_* capability imports (dio/       │  │
│  │    xpath/crypto)                         │  │
│  │  · async pump + NUL-string ABI + fuel    │  │
│  └──────────────────────────────────────────┘  │
├──────────── ABI v0 (authoritative: docs/abi.md) ┤
│   plugin.wasm (any language, grain.metadata)    │
│   · may only import "grain": 6 whitelisted caps │
│   · async fns compiled to state machines by PDK │
└────────────────────────────────────────────────┘
```

The essential difference from JS plugin engines: a plugin is a **compiled
binary**, not a script — reviewers can inspect it, the runtime exposes no
interpreter attack surface, and performance is near-native.

## Measured results (six verdicts, real network; reproduce via example/harness)

| # | Verdict | Result |
|---|---------|--------|
| 1 | Real-source fetch | latest 969ms / 20 items (JS-engine baseline 491ms/17, same class) |
| 2 | Async bridge | search 436ms, exact hit; async host results via event_wait direct write |
| 3 | Fuel termination | infinite loop trapped in 0~5ms (100k fuel precisely exhausted) |
| 4 | Panic isolation | trap caught by host; new instance loads normally in-process |
| 5 | Memory curve | ~2.3MB per plugin (20 instances: 118.6MB RSS) |
| 6 | Complex validation | Bilibili WBI signing (nav → key derivation → MD5) end-to-end, 176ms/20 items |

## Compared with alternatives

| | grain_engine | JS engine plugins | Declarative rules (Kazumi-style) |
|---|---|---|---|
| Plugin language | Any (WASM) | JS only | No code (config) |
| Expressiveness ceiling | Turing-complete, in-sandbox algorithms | Same | Low (no signing/computation) |
| Security | VM hard isolation + fuel | Soft-convention sandbox | Data, not executable (safest) |
| Ecosystem starting point | Greenfield | Mature | Mature |
| Runtime size | wasmtime ~11MB | engine ~1MB | none |

The trade-off, stated plainly: WASM means starting the ecosystem from zero in
exchange for expressiveness × security × a multi-language author pool. This
repository exists to make that greenfield buildable.

## Repository layout

```
lib/            Dart engine package (everything on the host side)
pdk/rust/       Rust PDK + proc macros + sample plugins + tooling
docs/           abi.md (authoritative protocol) · plugin-development.md (author guide)
example/        Runnable harness (one command reproduces all six verdicts)
```

## Roadmap

- [ ] PDKs: AssemblyScript (TS syntax) · Go/TinyGo
- [ ] Host capabilities: WebView escape hatch (CAPTCHA/browser-fingerprint
      scenarios), WebSocket
- [ ] `grain_now()` clock import (saves an HTTP Date round-trip)
- [ ] Hot plugin updates and source-signature verification
- [ ] pub.dev release

## Protocol & license

- ABI version: v0 (see [docs/abi.md](docs/abi.md); may change before v1)
- This repository: MIT (© 2026 HW)
- Runtime dependencies: [wasm_run](https://pub.dev/packages/wasm_run) (MIT) →
  [wasmtime](https://wasmtime.dev/) (Apache-2.0) — both mature open-source projects
