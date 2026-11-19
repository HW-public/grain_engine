import 'dart:convert';

/// grain_engine 类型化异常体系（ABI v0.1）。
///
/// 错误信封（guest last_error / 宿主能力层响应）：
/// `{"code":"plugin|http|abi|fuel|timeout","message":"...","detail"?:"...",
/// "httpStatus"?:int}`
/// pump 解析信封并抛出对应子类；缺失/未知 code 的旧式信封（v0 形状
/// `{"error":"..."}`）回退为 [PluginException] 兼容旧行为。

/// 基类：所有 grain_engine 异常的根。
class GrainException implements Exception {
  GrainException(this.pluginName, this.code, this.message, {this.detail});

  /// 插件名（宿主侧实例名）。
  final String pluginName;

  /// 错误码：`plugin` / `http` / `abi` / `fuel` / `timeout`。
  final String code;

  /// 人类可读的错误消息。
  final String message;

  /// 可选的补充细节（原始信封、堆栈摘要等）。
  final String? detail;

  @override
  String toString() {
    final d = detail == null ? '' : ' ($detail)';
    return '[$pluginName] $code: $message$d';
  }
}

/// 插件自身逻辑/业务错误（guest 侧 Error，code = `plugin`）。
class PluginLogicError extends GrainException {
  PluginLogicError(String pluginName, String message, {super.detail})
    : super(pluginName, 'plugin', message);
}

/// 宿主能力层错误（HTTP/xpath/crypto，code = `http`）。
class CapabilityError extends GrainException {
  CapabilityError(
    String pluginName,
    String message, {
    super.detail,
    this.httpStatus,
  }) : super(pluginName, 'http', message);

  /// 源站 HTTP 状态码（网络层失败时为 null）。
  final int? httpStatus;
}

/// 协议违例（code = `abi`）：缺导出、import 形状不符、非法请求等。
class AbiError extends GrainException {
  AbiError(String pluginName, String message, {super.detail})
    : super(pluginName, 'abi', message);
}

/// 沙箱/资源错误（code = `fuel`）：fuel 耗尽、trap、结果表打满、
/// 主机白名单拒绝。`fuel` 是「沙箱资源」类的信封判别码。
class SandboxError extends GrainException {
  SandboxError(String pluginName, String message, {super.detail})
    : super(pluginName, 'fuel', message);
}

/// 宿主泵超时（code = `timeout`）：pump 在超时预算内未完成。
class TimeoutError extends GrainException {
  TimeoutError(String pluginName, String message, {super.detail})
    : super(pluginName, 'timeout', message);
}

/// 兼容层：拿不到结构化 code 的旧式错误（v0 信封 `{"error":"..."}` 或
/// trap 无细节路径）保持旧行为，用本类抛出。
class PluginException implements Exception {
  PluginException(this.plugin, this.message);

  final dynamic plugin;
  final String message;

  String get pluginName => plugin is String ? plugin as String : plugin.name;

  @override
  String toString() => 'PluginException($pluginName): $message';
}

/// 从错误信封 JSON 构造类型化异常。非 JSON / 缺 code → [PluginException]。
Exception grainExceptionFromEnvelope(String pluginName, String envelopeJson) {
  Object? parsed;
  try {
    parsed = jsonDecode(envelopeJson);
  } catch (_) {
    parsed = null;
  }
  if (parsed is Map) {
    final code = parsed['code'];
    final message = parsed['message'];
    if (code is String && message is String) {
      final detail = parsed['detail'] is String
          ? parsed['detail'] as String
          : envelopeJson;
      final httpStatus = parsed['httpStatus'] is int
          ? parsed['httpStatus'] as int
          : null;
      switch (code) {
        case 'plugin':
          return PluginLogicError(pluginName, message, detail: detail);
        case 'http':
          return CapabilityError(
            pluginName,
            message,
            detail: detail,
            httpStatus: httpStatus,
          );
        case 'abi':
          return AbiError(pluginName, message, detail: detail);
        case 'fuel':
          return SandboxError(pluginName, message, detail: detail);
        case 'timeout':
          return TimeoutError(pluginName, message, detail: detail);
        default:
          return GrainException(pluginName, code, message, detail: detail);
      }
    }
  }
  // v0 旧式信封 {"error":"..."} 或无结构：兼容旧行为。
  return PluginException(pluginName, envelopeJson);
}
