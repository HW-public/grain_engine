# Security Policy / 安全政策

## 中文版

### 私下上报

如果你发现了 grain_engine 的安全问题（沙箱逃逸、内存安全问题、ABI 层漏洞等），
**请不要公开提交 issue 或在公开渠道讨论 PoC**。请通过 GitHub 私密漏洞上报通道：

**[GitHub Security Advisories（私密上报）](https://github.com/HW-public/grain_engine/security/advisories/new)**

> 该功能需在仓库 Settings → General → Security 中开启
> 「Private vulnerability reporting」（维护者操作，开启后即可匿名私密提交）。

### 响应时限

- 我们承诺在 **5 个工作日**内回复你的首次报告。
- 确认问题后，我们会与你沟通修复计划和时间表；修复发布时会公开致谢（除非你要求匿名）。

### 请不要公开 PoC

在修复发布之前，公开复现步骤或攻击代码会让所有仍在使用旧版本的用户暴露在
风险中。请给我们留出修复窗口。

### 已知边界（非安全目标内）

- **ABI v0 阶段**：协议尚处 v0（见 `docs/abi.md`），v1 前可能调整； ABI 本身
  的兼容性设计缺陷在冻结期后统一处理。
- **fuel 非完备资源限制**：fuel 机制用于掐死死循环（有限指令预算），但它不是
  完备的内存/CPU/带宽配额系统；恶意插件仍可能通过合法 import（如 HTTP）消耗
  宿主资源。
- **import 白名单依赖宿主实现**：插件可调用哪些 `grain_*` 能力由宿主注入，
  嵌入方自行决定暴露面。
- **fuel 默认关闭、且为累计预算**：`GrainPlugin.instantiate` 的燃料计量默认
  不启用，由宿主显式开启；`fuelAmount` 是实例累计预算（烧完后续调用全部
  trap），「单次调用上限」需用 `perCallFuel`（每次调用前自动充值）。
- **SSRF 边界**：插件可借宿主 HTTP 能力请求任意地址（插件模型固有，JS 引擎
  同样存在）；嵌入方可用 `allowedHosts` 白名单收窄，`null` 为不限制。
- **wasmtime 上游安全**：底层运行时安全漏洞请同时遵循
  [wasmtime 的安全政策](https://github.com/bytecodealliance/wasmtime/security)。

## English

**Private reporting:** If you find a security issue in grain_engine (sandbox escape,
memory safety, ABI-layer vulnerability), please do **not** file a public issue or
publish a PoC. Report it privately via
**[GitHub Security Advisories](https://github.com/HW-public/grain_engine/security/advisories/new)**
(requires the maintainer to enable "Private vulnerability reporting" in
Settings → General → Security).

**Response time:** We commit to replying to your initial report within **5 business
days**, and will share a fix plan and timeline once the issue is confirmed.

**No public PoC:** Please do not disclose reproduction steps until a fix has been
released, so users on older versions are not left exposed.

**Known boundaries (not security goals):** the ABI is at **v0** and may still change
before v1; **fuel-based termination is not a complete resource quota** (it bounds
instructions, not memory/bandwidth consumed through legitimate imports such as HTTP);
capability exposure depends on the embedding host. For vulnerabilities in the
underlying runtime, also follow
[wasmtime's security policy](https://github.com/bytecodealliance/wasmtime/security).
**Fuel is off by default** (opt-in via `instantiate`; `fuelAmount` is a cumulative
instance budget — use `perCallFuel` for per-call limits). **SSRF boundary:**
plugins can ask the host to fetch any URL (inherent to the plugin model); embedders
can restrict it with the `allowedHosts` allowlist (`null` = unrestricted).
