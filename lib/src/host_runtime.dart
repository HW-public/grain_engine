import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:crypto/crypto.dart';
import 'package:dio/dio.dart';
import 'package:wasm_run/wasm_run.dart';
import 'package:xpath_selector_html_parser/xpath_selector_html_parser.dart';

import 'wasm_binary.dart';
import 'exceptions.dart';
import 'native_bootstrap.dart';

/// ABI v0.1 — 权威文档为 `docs/abi.md`（v0 差异见文档「v0 → v0.1」节）。
///
/// 字符串 ABI：guest 分配「NUL 结尾」UTF-8 缓冲区，宿主读后按 strlen 解码；
/// 宿主写完也必须显式写 NUL（复用的 ABI 堆块不保证零填充）。
/// guest 侧 ABI 堆为 2MB 静态堆，块头自带长度，dealloc 忽略 len 参数。
///
/// Guest 导出：
/// - alloc(len: i32) -> i32 ptr；dealloc(ptr: i32, len: i32)
/// - metadata_json() -> i32 ptr（NUL 结尾；每次调用新分配，宿主读后
///   free_result(ptr)——guest 不缓存该指针）
/// - latest(page: i32) -> i32 guest handle（0x4000_0000 前缀，非 0；
///   0 = 立即失败，宿主调 last_error）
/// - search(kw_ptr: i32) -> i32 guest handle（kw 是宿主写入的 NUL 结尾
///   UTF-8 串，单参签名）
/// - detail(url_ptr: i32) -> i32 guest handle（可选导出；url 同上）
/// - watch(url_ptr: i32) -> i32 guest handle（可选导出；url 同上）
/// - take_result(handle) -> i32
///   （非 0 = NUL 结尾结果 JSON ptr，宿主读后经 free_result(ptr) 释放；
///   0 = 未完成或出错——宿主查 last_error() 区分，出错为终态会反复上报）
/// - free_result(ptr: i32)（按指针释放结果/错误/元数据缓冲；task 结果
///   同时移除 task；每个指针只调一次）
/// - last_error() -> i32 ptr（NUL 结尾错误信封
///   {"code":"plugin|http|abi|fuel|timeout","message","detail"?,"httpStatus"?}；
///   无错误返回 0；读后所有权归宿主，free_result 释放）
///
/// Host import（module "grain"），host handle 取值 (0, 0x4000_0000)：
/// - grain_log(level: i32, msg_ptr: i32, msg_len: i32)
///   （级别 0=debug 1=info 2=warn 3=error）
/// - grain_http_request(req_ptr: i32, req_len: i32) -> i32
///   （req JSON {"url","method","headers":{...},"body":""}）
/// - grain_xpath_query(html_ptr, html_len, xpath_ptr, xpath_len) -> i32
/// - grain_crypto_md5(ptr, len) -> i32 / grain_crypto_base64_encode(ptr, len) -> i32
/// - grain_event_wait(handle: i32, buf_ptr: i32, buf_cap: i32) -> i32
///   （0=未完成；-1=buf 太小（guest 翻倍重试）；-2=未知 handle；
///   >0=已把响应 JSON 原始字节写入 guest buf 的字节数）
///
/// 响应 JSON 约定：
/// - http 信封 {"status":int,"body":string,"headers":{string:string}}；header 值纯字符串
/// - xpath {"results":[string,...]}；md5/base64 {"value":"..."}
/// - 通用错误信封 {"code":...,"message":...,"detail"?,"httpStatus"?}
///   （旧式 v0 {"error":"..."} 仍被 guest PDK 容忍，宿主侧回退 PluginException）
///
/// 异步桥（判决项 #2 核心结论）：wasm_run 的 import 是纯同步 Dart 闭包，
/// guest 调用栈无法挂起，wasm_run 无 async import。guest 的 future 每次
/// take_result 被 poll 一次，Pending 时 guest 调 grain_event_wait 同步取结果：
/// 宿主把已完成 Future 的响应写进 guest 申请的 buf；未完成返回 0。真正的
/// 异步推进在宿主 pump：take_result 返 0 且无错误时 await 已登记的未来再重试。
class GrainPlugin {
  GrainPlugin._({
    required this.name,
    required this.module,
    required this.instance,
    required this.memory,
    required this.metadata,
    required Map<String, WasmFunction> functions,
    required this.initialFuel,
    required this.perCallFuel,
    required this.allowedHosts,
    required this.logger,
  }) : _functions = functions;

  final String name;
  final WasmModule module;
  final WasmInstance instance;
  final WasmMemory memory;

  /// 元数据（custom section "grain.metadata" 优先，其次 metadata_json）。
  final String metadata;
  final Map<String, WasmFunction> _functions;

  final Dio _dio = Dio(
    BaseOptions(
      connectTimeout: const Duration(seconds: 15),
      receiveTimeout: const Duration(seconds: 30),
    ),
  );

  /// host handle 空间，取值 (0, 0x4000_0000) 与 guest task handle 区分。
  final Map<int, _HostResult> _hostResults = {};
  int _nextHostHandle = 1;

  static const int guestHandleBase = 0x40000000;

  /// Compiles and instantiates a plugin with the grain imports wired up.
  ///
  /// 沙箱选项（默认全部关闭，由宿主显式开启）：
  /// - [consumeFuel]：启用燃料计量；[fuelAmount] 为实例初始预算（累计制，
  ///   烧完后续调用全部 trap）；[perCallFuel] 为每次 latest/search 调用
  ///   前自动充值，实现「单次调用燃料上限」语义。
  /// - [allowedHosts]：非空时插件 HTTP 请求仅允许这些主机（防 SSRF 借道
  ///   访问内网）；null 则不限制（插件模型默认可达任意地址，见 SECURITY.md）。
  /// - [logger]：该插件的日志接收器（默认 [DefaultGrainLogger]）；无论注入
  ///   什么实现，guest 日志与宿主侧事件都会先进每插件日志环 [logs]。
  static Future<GrainPlugin> instantiate(
    String path, {
    bool consumeFuel = false,
    int? fuelAmount,
    int? perCallFuel,
    Set<String>? allowedHosts,
    GrainLogger? logger,
  }) async {
    if (perCallFuel != null && !consumeFuel) {
      throw ArgumentError('perCallFuel requires consumeFuel: true');
    }
    await ensureBundledWasmRunNative();
    final bytes = await File(path).readAsBytes();
    final name = path
        .split(Platform.pathSeparator)
        .last
        .replaceAll('.wasm', '');
    final metadataSection = WasmBinary.customSection(bytes, 'grain.metadata');

    final module = await compileWasmModule(
      bytes,
      config: ModuleConfig(consumeFuel: consumeFuel),
    );
    final builder = module.builder();

    // 只注册模块实际声明的 import（如 panic_test 不引用 grain 模块）。
    final declared = module
        .getImports()
        .map((i) => '${i.module}.${i.name}')
        .toSet();
    void addImportIfDeclared(String name, WasmFunction fn) {
      if (declared.contains('grain.$name')) {
        builder.addImport('grain', name, fn);
      }
    }

    // Imports fire during guest execution, after instantiation completes,
    // so a late-bound reference to the plugin under construction is safe.
    late final GrainPlugin plugin;

    addImportIfDeclared(
      'grain_log',
      WasmFunction.voidReturn(
        (int level, int ptr, int len) => plugin._log(level, ptr, len),
        params: const [ValueTy.i32, ValueTy.i32, ValueTy.i32],
      ),
    );
    addImportIfDeclared(
      'grain_http_request',
      WasmFunction(
        (int reqPtr, int reqLen) => plugin._httpRequest(reqPtr, reqLen),
        params: const [ValueTy.i32, ValueTy.i32],
        results: const [ValueTy.i32],
      ),
    );
    addImportIfDeclared(
      'grain_xpath_query',
      WasmFunction(
        (int htmlPtr, int htmlLen, int xpathPtr, int xpathLen) =>
            plugin._xpathQuery(htmlPtr, htmlLen, xpathPtr, xpathLen),
        params: const [ValueTy.i32, ValueTy.i32, ValueTy.i32, ValueTy.i32],
        results: const [ValueTy.i32],
      ),
    );
    addImportIfDeclared(
      'grain_crypto_md5',
      WasmFunction(
        (int ptr, int len) => plugin._cryptoMd5(ptr, len),
        params: const [ValueTy.i32, ValueTy.i32],
        results: const [ValueTy.i32],
      ),
    );
    addImportIfDeclared(
      'grain_crypto_base64_encode',
      WasmFunction(
        (int ptr, int len) => plugin._cryptoBase64(ptr, len),
        params: const [ValueTy.i32, ValueTy.i32],
        results: const [ValueTy.i32],
      ),
    );
    addImportIfDeclared(
      'grain_event_wait',
      WasmFunction(
        (int handle, int bufPtr, int bufCap) =>
            plugin._eventWait(handle, bufPtr, bufCap),
        params: const [ValueTy.i32, ValueTy.i32, ValueTy.i32],
        results: const [ValueTy.i32],
      ),
    );

    final instance = await builder.build();
    final memory = instance.getMemory('memory');
    if (memory == null) {
      throw AbiError(name, 'plugin does not export linear memory');
    }

    final functions = <String, WasmFunction>{};
    for (final exportName in [
      'alloc',
      'dealloc',
      'metadata_json',
      'latest',
      'search',
      'detail',
      'watch',
      'take_result',
      'free_result',
      'last_error',
      'spin',
      'panic',
      'boom',
      'add',
    ]) {
      final fn = instance.getFunction(exportName);
      if (fn != null) functions[exportName] = fn;
    }
    // ABI 导出要求：只有真正使用 grain 能力（声明 grain import 或带 PDK 导出）
    // 的插件才强制 alloc/dealloc/take_result/free_result；裸探针模块
    // （如 panic_test 只有 spin/panic/add）放行。
    final usesGrainAbi =
        module.getImports().any((i) => i.module == 'grain') ||
        functions.keys.any(
          (k) =>
              k == 'metadata_json' ||
              k == 'latest' ||
              k == 'search' ||
              k == 'take_result' ||
              k == 'last_error',
        );
    if (usesGrainAbi) {
      for (final required in [
        'alloc',
        'dealloc',
        'take_result',
        'free_result',
      ]) {
        if (!functions.containsKey(required)) {
          // 协议违例（缺导出）→ AbiError。
          throw AbiError(
            name,
            'missing required export "$required"',
            detail: 'declared grain ABI but export not found',
          );
        }
      }
    }

    // metadata 来源：custom section 优先；否则读 metadata_json
    // （NUL 结尾，每次调用新分配，宿主读后 free_result）。
    var metadata = metadataSection ?? '';
    if (metadata.isEmpty && functions.containsKey('metadata_json')) {
      final ptr = (functions['metadata_json']!([]).first as num).toInt();
      if (ptr > 0) {
        try {
          metadata = _decodeCstr(memory.view, ptr);
        } finally {
          functions['free_result']!([ptr]);
        }
      }
    }

    if (fuelAmount != null) {
      (builder.fuel() ?? instance.fuel())?.addFuel(fuelAmount);
    }
    plugin = GrainPlugin._(
      name: name,
      module: module,
      instance: instance,
      memory: memory,
      metadata: metadata,
      functions: functions,
      initialFuel: fuelAmount,
      perCallFuel: perCallFuel,
      allowedHosts: allowedHosts,
      logger: logger ?? DefaultGrainLogger(),
    );
    return plugin;
  }

  /// 实例初始燃料预算（null = 未启用燃料）。
  final int? initialFuel;

  /// 每次调用前充值的燃料（null = 不自动充值）。
  final int? perCallFuel;

  /// HTTP 主机白名单（null = 不限制）。
  final Set<String>? allowedHosts;

  /// 待消费宿主结果表上限：防插件狂发请求不消费导致的宿主内存膨胀。
  static const int maxPendingHostResults = 64;

  /// 每插件日志环上限：满了丢最旧。
  static const int maxLogEntries = 500;

  /// 该插件的日志接收器（instantiate 注入，可替换）。
  final GrainLogger logger;

  final List<LogEntry> _logRing = [];

  /// 排障时间线：guest grain_log 与该插件的宿主侧事件（http 异常、
  /// event_wait 错误、fuel/trap）都进这一个环，按时间先后排列。
  List<LogEntry> get logs => List.unmodifiable(_logRing);

  /// 写日志环 + 回调 logger（双写，logger 可替换）。logger 自身异常
  /// 不得击穿宿主（import 回调里抛异常会 segfault 整个进程）。
  void _record(int level, String msg, {Object? error}) {
    if (_logRing.length >= maxLogEntries) _logRing.removeAt(0);
    _logRing.add(
      LogEntry(
        ts: DateTime.now(),
        level: level,
        msg: msg,
        error: error?.toString(),
      ),
    );
    try {
      logger.log(name, level, msg, error: error);
    } catch (_) {
      // 日志器故障静默：绝不在 wasm import 回调路径上抛出。
    }
  }

  /// 包装 guest 导出调用：trap（含 fuel 耗尽）记录进日志环后原样抛出。
  R _guard<R>(String what, R Function() call) {
    try {
      return call();
    } catch (e) {
      _record(3, 'guest trap on $what', error: e);
      rethrow;
    }
  }

  void _refuel() {
    final amount = perCallFuel;
    if (amount == null) return;
    instance.fuel()?.addFuel(amount);
  }

  /// Calls guest `latest(page)` and pumps until the guest handle resolves.
  /// [timeout] 为泵的总预算（默认 30s），超时抛 [TimeoutError]。
  Future<String> callLatest(int page, {Duration? timeout}) async {
    final fn = _functions['latest'];
    if (fn == null) {
      throw AbiError(name, 'no latest export', detail: 'plugin_fn latest?');
    }
    _refuel();
    final handle = _guard('latest', () => (fn([page]).first as num).toInt());
    return pump(handle, timeout: timeout);
  }

  /// Calls guest [exportName] with one string argument and pumps until the
  /// guest handle resolves.
  ///
  /// 入参为 NUL 结尾 UTF-8 串：复用堆块不保证零填充，必须显式写 NUL。
  /// 缺导出抛 [AbiError]（`detail`/`watch` 是可选导出，行为与缺 `search` 一致）。
  Future<String> _callWithCString(
    String exportName,
    String arg, {
    Duration? timeout,
  }) async {
    final fn = _functions[exportName];
    if (fn == null) {
      throw AbiError(
        name,
        'no $exportName export',
        detail: 'plugin_fn $exportName?',
      );
    }
    final bytes = utf8.encode(arg);
    final ptr = (_functions['alloc']!([bytes.length + 1]).first as num).toInt();
    int handle;
    try {
      memory.view.setRange(ptr, ptr + bytes.length, bytes);
      memory.view[ptr + bytes.length] = 0;
      _refuel();
      handle = _guard(exportName, () => (fn([ptr]).first as num).toInt());
    } finally {
      _functions['dealloc']!([ptr, bytes.length + 1]);
    }
    return pump(handle, timeout: timeout);
  }

  /// Calls guest `search(kw)` and pumps until the guest handle resolves.
  Future<String> callSearch(String keyword, {Duration? timeout}) =>
      _callWithCString('search', keyword, timeout: timeout);

  /// Calls guest `detail(url)` and pumps until the guest handle resolves.
  /// 可选导出：插件未实现时抛 [AbiError]。
  Future<String> callDetail(String url, {Duration? timeout}) =>
      _callWithCString('detail', url, timeout: timeout);

  /// Calls guest `watch(url)` and pumps until the guest handle resolves.
  /// 可选导出：插件未实现时抛 [AbiError]。
  Future<String> callWatch(String url, {Duration? timeout}) =>
      _callWithCString('watch', url, timeout: timeout);

  /// Host-side pump: poll take_result; 0 = pending or error (distinguished
  /// via last_error); while pending, await registered host futures (real
  /// async progress on the Dart event loop) and retry.
  ///
  /// [timeout] 为总预算（默认 30s）：每次 await 宿主 Future 只等到剩余
  /// 预算，烧完抛 [TimeoutError]。错误信封（ABI v0.1）解析为类型化异常。
  Future<String> pump(int guestHandle, {Duration? timeout}) async {
    final budget = timeout ?? const Duration(seconds: 30);
    final sw = Stopwatch()..start();
    while (true) {
      final ptr = _guard(
        'take_result',
        () => (_functions['take_result']!([guestHandle]).first as num).toInt(),
      );
      if (ptr > 0) {
        try {
          return _decodeCstr(memory.view, ptr);
        } finally {
          // 按指针释放 task 结果缓冲（每个指针只调一次）。
          _functions['free_result']!([ptr]);
        }
      }
      // ptr == 0：未完成或出错。last_error 非空即终态错误。
      final errPtr = (_functions['last_error']!([]).first as num).toInt();
      if (errPtr > 0) {
        final errJson = _decodeCstr(memory.view, errPtr);
        _functions['free_result']!([errPtr]);
        final ex = grainExceptionFromEnvelope(name, errJson);
        throw ex;
      }
      final pending = _hostResults.values
          .where((r) => !r.isCompleted)
          .map((r) => r.done.future)
          .toList(growable: false);
      if (pending.isEmpty) {
        throw PluginException(
          this,
          'guest handle ${guestHandle.toRadixString(16)} pending '
          'but no host work registered',
        );
      }
      final remaining = budget - sw.elapsed;
      if (remaining <= Duration.zero) {
        throw TimeoutError(
          name,
          'pump timed out after ${budget.inSeconds}s',
          detail: 'guest handle ${guestHandle.toRadixString(16)} still pending',
        );
      }
      try {
        await Future.any(pending).timeout(remaining);
      } on TimeoutException {
        throw TimeoutError(
          name,
          'pump timed out after ${budget.inSeconds}s',
          detail: 'guest handle ${guestHandle.toRadixString(16)} still pending',
        );
      }
      // 不在此清理已完成结果：guest 尚未经 event_wait 消费，
      // 提前移除会导致 unknown host handle。清理发生在 event_wait 成功投递时。
    }
  }

  /// 从「NUL 结尾」ABI 缓冲区解码 UTF-8 字符串。
  static String _decodeCstr(Uint8List view, int ptr) {
    var len = 0;
    while (ptr + len < view.length && view[ptr + len] != 0) {
      len++;
    }
    return utf8.decode(view.sublist(ptr, ptr + len));
  }

  Uint8List readBytes(int ptr, int len) =>
      Uint8List.sublistView(memory.view, ptr, ptr + len);

  String readStr(int ptr, int len) => utf8.decode(readBytes(ptr, len));

  void _log(int level, int ptr, int len) {
    // import 回调里抛异常会直接 segfault 整个进程（wasm_run 实测），必须兜死。
    try {
      _record(level, readStr(ptr, len));
    } catch (e) {
      _record(3, 'log decode failed', error: e);
    }
  }

  int _httpRequest(int reqPtr, int reqLen) {
    // 全函数兜底：readStr 解码失败等异常穿出 FFI 会 segfault 宿主进程。
    try {
      final reqJson = readStr(reqPtr, reqLen);
      if (_hostResults.length >= maxPendingHostResults) {
        // 结果表打满（插件发请求不消费）：拒绝新请求，防宿主内存膨胀。
        // 沙箱资源冲突 → code "fuel"（SandboxError）。
        _record(2, 'host result table full, rejecting request: $reqJson');
        return _completeSync(
          jsonEncode({
            'code': 'fuel',
            'message': 'host result table full (cap $maxPendingHostResults)',
          }),
        );
      }
      final handle = _nextHostHandle++;
      assert(handle < guestHandleBase);
      final result = _HostResult();
      _hostResults[handle] = result;
      unawaited(_runHttp(result, reqJson));
      return handle;
    } catch (e) {
      // 插件发来的请求形状不符 = 协议违例 → code "abi"。
      _record(3, 'http request decode failed', error: e);
      return _completeSync(
        jsonEncode({'code': 'abi', 'message': 'invalid request: $e'}),
      );
    }
  }

  Future<void> _runHttp(_HostResult result, String reqJson) async {
    try {
      final req = jsonDecode(reqJson) as Map<String, dynamic>;
      final url = req['url'] as String;
      // SSRF 边界：白名单非空时仅允许清单内主机（沙箱策略 → SandboxError）。
      final hosts = allowedHosts;
      if (hosts != null && !hosts.contains(Uri.parse(url).host)) {
        _record(2, 'host not allowed: ${Uri.parse(url).host}');
        result.complete(
          jsonEncode({
            'code': 'fuel',
            'message': 'host not allowed: ${Uri.parse(url).host}',
          }),
        );
        return;
      }
      final response = await _dio.request<dynamic>(
        url,
        data: (req['body'] as String?)?.isNotEmpty == true ? req['body'] : null,
        options: Options(
          method: (req['method'] as String?) ?? 'GET',
          headers: (req['headers'] as Map?)?.cast<String, dynamic>(),
          responseType: ResponseType.plain,
        ),
      );
      // header 值必须是纯字符串（guest 端 as_str 直取），dio 给的是数组。
      final headerMap = <String, String>{
        for (final e in response.headers.map.entries) e.key: e.value.join(', '),
      };
      // dio 把 header 名规范为小写，而 ABI 约定插件按 "Date" 查找（wasm 内
      // 无时钟，靠它推导时间）——补一份规范大小写键。
      final date = headerMap['date'];
      if (date != null && !headerMap.containsKey('Date')) {
        headerMap['Date'] = date;
      }
      result.complete(
        jsonEncode({
          'status': response.statusCode ?? 0,
          'body': response.data?.toString() ?? '',
          'headers': headerMap,
        }),
      );
    } catch (e) {
      _record(3, 'http request failed', error: e);
      // 能力层失败：能拿到源站状态码就带上（guest 可区分风控/超时）。
      final status = e is DioException ? e.response?.statusCode : null;
      result.complete(
        jsonEncode({
          'code': 'http',
          'message': e.toString(),
          if (status != null) 'httpStatus': status,
        }),
      );
    }
  }

  int _xpathQuery(int htmlPtr, int htmlLen, int xpathPtr, int xpathLen) {
    final handle = _nextHostHandle++;
    final result = _HostResult();
    _hostResults[handle] = result;
    try {
      final html = readStr(htmlPtr, htmlLen);
      final xpath = readStr(xpathPtr, xpathLen);
      final query = HtmlXPath.html(html).query(xpath);
      result.complete(
        jsonEncode({
          'results': [for (final node in query.nodes) node.text ?? ''],
        }),
      );
    } catch (e) {
      // 非法 xpath/html 入参 = 协议违例 → code "abi"。
      _record(3, 'xpath query failed', error: e);
      result.complete(
        jsonEncode({'code': 'abi', 'message': 'xpath failed: $e'}),
      );
    }
    return handle;
  }

  int _cryptoMd5(int ptr, int len) {
    try {
      final digest = md5.convert(readBytes(ptr, len)).toString();
      return _completeSync(jsonEncode({'value': digest}));
    } catch (e) {
      _record(3, 'md5 failed', error: e);
      return _completeSync(
        jsonEncode({'code': 'abi', 'message': 'md5 failed: $e'}),
      );
    }
  }

  int _cryptoBase64(int ptr, int len) {
    try {
      return _completeSync(
        jsonEncode({'value': base64Encode(readBytes(ptr, len))}),
      );
    } catch (e) {
      _record(3, 'base64 failed', error: e);
      return _completeSync(
        jsonEncode({'code': 'abi', 'message': 'base64 failed: $e'}),
      );
    }
  }

  int _completeSync(String responseJson) {
    final handle = _nextHostHandle++;
    _hostResults[handle] = _HostResult()..complete(responseJson);
    return handle;
  }

  /// 同步闭包无法挂起：已完成则把响应原始字节写进 guest buf；未完成 0。
  /// 成功投递后移除登记：guest 的 HostOp 一旦取到结果便不再使用该 handle。
  /// 注意：import 回调里抛异常会直接 segfault 整个进程（wasm_run 实测），
  /// 这里兜成 -2 未知 handle，错误细节走宿主日志。
  int _eventWait(int handle, int bufPtr, int bufCap) {
    try {
      final result = _hostResults[handle];
      if (result == null) return -2;
      if (!result.isCompleted) return 0;
      final bytes = utf8.encode(result.response);
      if (bytes.length > bufCap) return -1;
      memory.view.setRange(bufPtr, bufPtr + bytes.length, bytes);
      _hostResults.remove(handle);
      return bytes.length;
    } catch (e) {
      // -2 兜住：错误细节进该插件日志环（排障时间线），guest 侧只拿到未知 handle。
      _record(3, 'event-wait error', error: e);
      return -2;
    }
  }

  /// Raw export access for the fuel / panic verdicts.
  WasmFunction? export(String name) => _functions[name];

  void dispose() => instance.dispose();
}

/// One host-side import result: a completion signal for the host pump plus
/// the response JSON served through grain_event_wait.
class _HostResult {
  final Completer<void> done = Completer<void>();
  late final String _response;

  bool get isCompleted => done.isCompleted;
  String get response => _response;

  void complete(String responseJson) {
    _response = responseJson;
    done.complete();
  }
}

/// 日志级别（与 ABI grain_log 的 level 参数一致）：
/// 0=debug 1=info 2=warn 3=error。
abstract class GrainLogger {
  /// [plugin] 为插件实例名；[level] 见上；[error] 是可选的关联异常/对象。
  void log(String plugin, int level, String msg, {Object? error});
}

/// 默认日志器：debugPrint 输出 `[plugin] msg`（error 附加在末尾）。
class DefaultGrainLogger implements GrainLogger {
  const DefaultGrainLogger();

  static const List<String> _levelNames = ['DEBUG', 'INFO', 'WARN', 'ERROR'];

  @override
  void log(String plugin, int level, String msg, {Object? error}) {
    final lvl = level >= 0 && level < _levelNames.length
        ? _levelNames[level]
        : level.toString();
    // ignore: avoid_print
    print('[$plugin/$lvl] $msg${error == null ? '' : ' ($error)'}');
  }
}

/// 每插件日志环的一条记录。
class LogEntry {
  const LogEntry({
    required this.ts,
    required this.level,
    required this.msg,
    this.error,
  });

  final DateTime ts;
  final int level;
  final String msg;
  final String? error;

  @override
  String toString() {
    final e = error == null ? '' : ' ($error)';
    return '${ts.toIso8601String()} [$level] $msg$e';
  }
}
