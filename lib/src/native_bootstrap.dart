import 'dart:convert';
import 'dart:io';

/// wasm_run 官方 DLL 查找顺序（ffi/io.dart 实证）：
/// 1. 环境变量 `WASM_RUN_DART_DYNAMIC_LIBRARY`
/// 2. 裸名加载（依赖 PATH）
/// 3. `{项目根}/.dart_tool/wasm_run/wasm_run_dart.dll` ← 本文件利用的落点
///
/// 本引导把包内 `native/windows-x64/` 内嵌的 DLL 拷贝到该目录，
/// 使消费方无需运行 wasm_run 官方 setup 命令、无需下载。
/// 目前仅内嵌 Windows x64；其他平台静默跳过，回落到 wasm_run 自身查找。
Future<void> ensureBundledWasmRunNative() async {
  if (_done) return;
  _done = true;

  if (!Platform.isWindows) return;
  // 用户显式指定了库路径时尊重指定值，不做任何操作。
  if (Platform.environment.containsKey('WASM_RUN_DART_DYNAMIC_LIBRARY')) return;

  try {
    final bundled = await _locateBundledDll();
    if (bundled == null || !bundled.existsSync()) return;

    final root = _projectRoot(Directory.current.uri);
    if (root == null) return;
    final dest = File.fromUri(
      root.resolve('.dart_tool/wasm_run/wasm_run_dart.dll'),
    );
    if (dest.existsSync() && dest.lengthSync() == bundled.lengthSync()) return;

    dest.createSync(recursive: true);
    await bundled.copy(dest.path);
  } catch (_) {
    // 引导失败不致命：wasm_run 仍可按其自身顺序查找
    //（setup 命令落点 / 环境变量 / PATH）。
  }
}

bool _done = false;

const _pkgConfigFile = '.dart_tool/package_config.json';
const _bundledRelPath = 'native/windows-x64/wasm_run_dart.dll';

/// 从项目根的 package_config.json 反查本包（grain_engine）的根目录。
/// 不能用 `Isolate.resolvePackageUri`：package: URI 一律映射到 `lib/` 下，
/// 而 DLL 按 pub 惯例放在包根的 `native/`，须直接读 package_config 的
/// rootUri（path / git / pub 缓存三种引用方式均适用）。
Future<File?> _locateBundledDll() async {
  var dir = Directory.current.uri;
  File? cfg;
  do {
    final f = File.fromUri(dir.resolve(_pkgConfigFile));
    if (f.existsSync()) {
      cfg = f;
      break;
    }
    // ignore: parameter_assignments
  } while (dir != (dir = dir.resolve('../')));
  if (cfg == null) return null;

  final json = jsonDecode(await cfg.readAsString());
  if (json is! Map || json['packages'] is! List) return null;
  for (final p in json['packages'] as List) {
    if (p is Map && p['name'] == 'grain_engine' && p['rootUri'] is String) {
      var root = cfg.uri.resolve(p['rootUri'] as String);
      // path 依赖的 rootUri 常无尾斜杠（如 "../../grain_engine"），
      // Uri.resolve 会把末段当文件名吃掉——补斜杠后再拼相对路径。
      if (!root.path.endsWith('/')) {
        root = root.replace(path: '${root.path}/');
      }
      return File.fromUri(root.resolve(_bundledRelPath));
    }
  }
  return null;
}

/// 自 [start] 向上寻找含 package_config.json 的项目根（与 wasm_run
/// libBuildOutDir 同算法，保证拷贝到 wasm_run 实际查找的目录）。
Uri? _projectRoot(Uri start) {
  var root = start;
  do {
    if (FileSystemEntity.isFileSync(
      root.resolve(_pkgConfigFile).toFilePath(),
    )) {
      return root;
    }
    // ignore: parameter_assignments
  } while (root != (root = root.resolve('../')));
  return null;
}
