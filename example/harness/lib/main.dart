import 'dart:convert';
import 'dart:io';

import 'package:flutter/widgets.dart';
import 'package:grain_engine/grain_engine.dart';

/// 结果输出路径（可用 HARNESS_OUT 环境变量覆盖）。
final outputPath = Platform.environment['HARNESS_OUT'] ?? 'harness_output.txt';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  final buffer = StringBuffer();

  void emit(String line) {
    // ignore: avoid_print
    print(line);
    buffer.writeln(line);
  }

  emit('=== grain_engine example harness (wasm_run host) ===');
  emit('dart: ${Platform.version}');

  final guestDir = Directory(_wasmDirArg() ?? _defaultGuestDir());
  final wasmFiles = <String>[];
  if (guestDir.existsSync()) {
    wasmFiles.addAll(
      guestDir
          .listSync()
          .whereType<File>()
          .where((f) => f.path.endsWith('.wasm'))
          .map((f) => f.path),
    );
    wasmFiles.sort();
  }

  if (wasmFiles.isEmpty) {
    emit('GUEST_NOT_READY: no .wasm found in ${guestDir.path}');
    await _finish(buffer, emit);
    return;
  }
  emit('guest artifacts: ${wasmFiles.join(', ')}');

  // 默认插件（验证项 1/2/3/5）：优先 360zy（API 型插件），
  // 其次任意非 bili/非 panic 产物。
  final defaultPath = wasmFiles.firstWhere(
    (p) => p.toLowerCase().contains('360zy'),
    orElse: () => wasmFiles.firstWhere(
      (p) =>
          !p.toLowerCase().contains('bili') &&
          !p.toLowerCase().contains('panic'),
      orElse: () => wasmFiles.first,
    ),
  );

  GrainPlugin? plugin;
  try {
    plugin = await GrainPlugin.instantiate(defaultPath);
    emit('loaded plugin: ${plugin.name}');
    emit(
      'custom section grain.metadata: '
      '${plugin.metadata.isEmpty ? '(empty)' : plugin.metadata}',
    );

    // 验证项 1：latest 计时。
    final sw = Stopwatch()..start();
    final latestJson = await plugin.callLatest(1);
    sw.stop();
    emit('');
    emit(
      '[1] latest(page=1): ${sw.elapsedMilliseconds} ms, '
      '${_countItems(latestJson)} items',
    );
    emit('    raw: ${_truncate(latestJson)}');

    // 验证项 2：search。
    final sw2 = Stopwatch()..start();
    final searchJson = await plugin.callSearch('孤独摇滚');
    sw2.stop();
    emit('');
    emit(
      '[2] search("孤独摇滚"): ${sw2.elapsedMilliseconds} ms, '
      '${_countItems(searchJson)} items',
    );
    emit('    raw: ${_truncate(searchJson)}');

    // 验证项 3：fuel（用 panic_test 的 spin() 死循环验证掐死；无则跳过）。
    final panicPath = wasmFiles.firstWhere(
      (p) => p.toLowerCase().contains('panic'),
      orElse: () => defaultPath,
    );
    emit('');
    await _fuelVerdict(panicPath, emit);

    // 验证项 4：panic 恢复（trap 后宿主可继续装载其他插件）。
    emit('');
    await _panicVerdict(panicPath, emit);

    // 验证项 5：内存曲线 (1 / 5 / 10 / 20 实例)。
    emit('');
    await _memoryVerdict(defaultPath, emit);

    // 验证项 6：bili-wbi 全流程。
    emit('');
    final biliPath = wasmFiles.firstWhere(
      (p) => p.toLowerCase().contains('bili'),
      orElse: () => '',
    );
    if (biliPath.isEmpty) {
      emit('[6] bili-wbi: N/A (no bili plugin artifact)');
    } else {
      await _biliVerdict(biliPath, emit);
    }
  } catch (e, s) {
    emit('HARNESS_ERROR: $e\n$s');
  } finally {
    plugin?.dispose();
  }

  await _finish(buffer, emit);
}

/// 未显式指定时的默认插件产物目录：从当前目录向上找本仓库
/// pdk/rust 的 wasm32 release 产物目录。
String _defaultGuestDir() {
  var dir = Directory.current;
  for (var i = 0; i < 6; i++) {
    final candidate = Directory(
      '${dir.path}${Platform.pathSeparator}pdk${Platform.pathSeparator}rust'
      '${Platform.pathSeparator}target${Platform.pathSeparator}'
      'wasm32-unknown-unknown${Platform.pathSeparator}release',
    );
    if (candidate.existsSync()) return candidate.path;
    final parent = dir.parent;
    if (parent.path == dir.path) break;
    dir = parent;
  }
  return Directory.current.path;
}

Future<void> _fuelVerdict(String path, void Function(String) emit) async {
  try {
    final plugin = await GrainPlugin.instantiate(
      path,
      consumeFuel: true,
      fuelAmount: 100000,
    );
    final spin = plugin.export('spin');
    if (spin == null) {
      emit('[3] fuel: N/A — no "spin" export in this plugin');
      return;
    }
    try {
      final sw = Stopwatch()..start();
      try {
        spin([]);
        sw.stop();
        emit(
          '[3] fuel: spin COMPLETED within 100000 fuel '
          '(${sw.elapsedMilliseconds} ms) — unexpected',
        );
      } catch (e) {
        sw.stop();
        emit('[3] fuel: trapped after ${sw.elapsedMilliseconds} ms: $e');
      }
      final fuel = plugin.instance.fuel();
      emit(
        '    fuelConsumed: ${fuel?.fuelConsumed()}, '
        'fuelAdded: ${fuel?.fuelAdded()}',
      );
    } finally {
      plugin.dispose();
    }
  } catch (e) {
    emit('[3] fuel: N/A — wasm_run fuel API failed: $e');
  }
}

Future<void> _panicVerdict(String path, void Function(String) emit) async {
  final panicPlugin = await GrainPlugin.instantiate(path);
  final panicFn = panicPlugin.export('panic') ?? panicPlugin.export('boom');
  if (panicFn == null) {
    emit('[4] panic recovery: N/A (guest exports no "panic"/"boom" function)');
    panicPlugin.dispose();
    return;
  }
  try {
    panicFn([]);
    emit('[4] panic recovery: trap function returned normally (unexpected)');
  } catch (e) {
    emit('[4] panic recovery: trap caught by host: $e');
  }
  panicPlugin.dispose();

  // 宿主必须保持可用：trap 之后新实例仍可正常装载。
  try {
    final fresh = await GrainPlugin.instantiate(path);
    final metadata = fresh.metadata;
    fresh.dispose();
    emit(
      '    host alive after trap: fresh instance metadata '
      '${metadata.isEmpty ? '(empty)' : 'ok (${metadata.length} chars)'}',
    );
  } catch (e) {
    emit('    host BROKEN after trap: $e');
  }
}

Future<void> _memoryVerdict(String path, void Function(String) emit) async {
  final loader = PluginLoader();
  emit('[5] memory curve (RSS via ProcessInfo.currentRss):');
  for (final target in [1, 5, 10, 20]) {
    await loader.loadUpTo(path, target);
  }
  var previous = 0;
  for (final sample in loader.rssSamples) {
    if (const [1, 5, 10, 20].contains(sample.$1)) {
      final delta = sample.$2 - previous;
      previous = sample.$2;
      emit(
        '    ${sample.$1} plugins: RSS '
        '${(sample.$2 / 1024 / 1024).toStringAsFixed(1)} MB '
        '(+${(delta / 1024 / 1024).toStringAsFixed(1)} MB)',
      );
    }
  }
  loader.disposeAll();
}

Future<void> _biliVerdict(String path, void Function(String) emit) async {
  try {
    final plugin = await GrainPlugin.instantiate(path);
    try {
      emit('[6] bili-wbi plugin: ${plugin.name}');
      emit('    metadata: ${plugin.metadata}');
      final sw = Stopwatch()..start();
      final json = await plugin.callLatest(1);
      sw.stop();
      emit(
        '    latest: ${sw.elapsedMilliseconds} ms, '
        '${_countItems(json)} items',
      );
      emit('    raw: ${_truncate(json)}');
      // 全流程：nav 取 key + Date 推导 wts → mixin 签名 → wbi search。
      // 机房 IP 可能被风控 -412/-352，如实记录。
      try {
        final sw2 = Stopwatch()..start();
        final searchJson = await plugin.callSearch('鬼灭之刃');
        sw2.stop();
        emit(
          '    search("鬼灭之刃"): ${sw2.elapsedMilliseconds} ms, '
          '${_countItems(searchJson)} items',
        );
        emit('    raw: ${_truncate(searchJson)}');
      } catch (e) {
        emit('    search: FAILED (可能是 -412/-352 风控): $e');
      }
    } finally {
      plugin.dispose();
    }
  } catch (e) {
    emit('[6] bili-wbi: FAILED: $e');
  }
}

/// `--wasm-dir <path>` / `--wasm-dir=<path>` 启动参数或 WASM_DIR 环境变量
/// 可覆盖插件产物目录。环境变量优先（Windows GUI runner 不透传 argv）。
String? _wasmDirArg() {
  final env = Platform.environment['WASM_DIR'];
  if (env != null && env.isNotEmpty) return env;
  final args = Platform.executableArguments;
  final i = args.indexOf('--wasm-dir');
  if (i >= 0 && i + 1 < args.length) return args[i + 1];
  final inline = args
      .map((a) => a.startsWith('--wasm-dir=') ? a.substring(11) : null)
      .firstWhere((a) => a != null, orElse: () => null);
  return inline;
}

int _countItems(String json) {
  try {
    final decoded = jsonDecode(json);
    if (decoded is List) return decoded.length;
    if (decoded is Map) {
      for (final key in ['data', 'items', 'results', 'list']) {
        final value = decoded[key];
        if (value is List) return value.length;
      }
    }
    return -1;
  } catch (_) {
    return -1;
  }
}

String _truncate(String s, [int max = 300]) =>
    s.length <= max ? s : '${s.substring(0, max)}... (${s.length} chars)';

Future<void> _finish(StringBuffer buffer, void Function(String) emit) async {
  emit('=== harness done ===');
  try {
    await File(outputPath).writeAsString(buffer.toString());
    // ignore: avoid_print
    print('(results written to $outputPath)');
  } catch (e) {
    // ignore: avoid_print
    print('(failed to write $outputPath: $e)');
  }
  exit(0);
}
