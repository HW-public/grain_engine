/// grain_engine —— Flutter/Dart 原生 WASM 插件引擎。
///
/// 插件以 WASM 模块形式装载：任意语言编写，VM 级沙箱运行，
/// 宿主能力经 `grain` import 白名单注入，异步经 event_wait 直写桥推进。
///
/// 用法：
/// ```dart
/// final plugin = await GrainPlugin.instantiate('plugin_360zy.wasm');
/// final latest = await plugin.callLatest(1);
/// final search = await plugin.callSearch('孤独摇滚');
/// plugin.dispose();
/// ```
library;

export 'src/host_runtime.dart';
export 'src/plugin_loader.dart';
export 'src/exceptions.dart';
