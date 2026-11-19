# Bundled native binaries / 内嵌原生库声明

grain_engine ships prebuilt copies of the wasm_run native library so that
consumers do not need to download anything at setup time.
（grain_engine 内嵌 wasm_run 原生库的预编译副本，消费方无需在配置阶段下载。）

## native/windows-x64/wasm_run_dart.dll

- Upstream: [wasm_run](https://github.com/juancastillo0/wasm_run),
  release `wasm_run-v0.1.0+2`, artifact `other.tar.gz` → `windows-x64/`
- License: MIT (see upstream LICENSE);
  bundles [wasmtime](https://github.com/bytecodealliance/wasmtime),
  license Apache-2.0 WITH LLVM-exception
- Loaded via wasm_run's official fallback path
  (`{project}/.dart_tool/wasm_run/`, see `lib/src/native_bootstrap.dart`)
- 与包内 wasm_run Dart 代码（0.1.0+2）版本锁定，不会出现库/代码版本错配。

macOS / Linux / Android 暂未内嵌：桌面端仍可用 wasm_run 官方 setup 命令
（`dart run wasm_run:setup`），Android 需要插件化打包（见 Roadmap）。
