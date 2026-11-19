# grain_engine ABI v0.2（终版权威）

宿主（`lib/src/host_runtime.dart`）与 guest（Rust PDK）共同遵守的 ABI。
本文档为唯一权威；两侧实现均以本文为准。

## 总览

- 目标：`wasm32-unknown-unknown`，无 WASI。宿主能力全部经 `import "grain"` 注入。
- 字符串 ABI：所有跨边界缓冲区均为 **NUL 结尾 UTF-8 串**（guest 分配；宿主读前
  strlen；宿主写入必须显式写 NUL——复用的 ABI 堆块不保证零填充）。
- guest 侧 ABI 堆为 2MB 静态堆（first-fit 空闲链表），块头自带长度，
  `dealloc` 忽略 len 参数。

## Guest 导出（11 项，与宿主严格校验清单一一对应）

| 导出 | 说明 |
| --- | --- |
| `alloc(len) -> ptr` | ABI 堆分配 |
| `dealloc(ptr, len)` | 释放（len 仅 ABI 兼容保留，长度从块头读） |
| `metadata_json() -> ptr` | NUL 结尾 JSON；**每次调用新分配**（宿主读完调 `free_result(ptr)`，缓存会悬垂） |
| `latest(page) -> handle` | 0 = 立即失败（宿主调 `last_error`）；非 0 = guest handle |
| `search(kw_ptr) -> handle` | 同上；kw 是宿主写入的 NUL 结尾 UTF-8 串 |
| `detail(url_ptr) -> handle` | **v0.2 可选导出**；url 同上；结果 JSON 契约见下节。插件未实现时宿主 `callDetail` 抛 `AbiError` |
| `watch(url_ptr) -> handle` | **v0.2 可选导出**；url 同上；结果 JSON 契约见下节。插件未实现时宿主 `callWatch` 抛 `AbiError` |
| `take_result(handle) -> ptr` | 0 = 未完成或出错（**宿主应查 `last_error()` 区分**）；非 0 = NUL 结尾 JSON 指针，读完 `free_result(ptr)` |
| `last_error() -> ptr` | NUL 结尾**错误信封** `{"code":"plugin\|http\|abi\|fuel\|timeout","message","detail"?,"httpStatus"?}`；无错误返回 0；读后所有权归宿主（free_result 释放） |
| `free_result(ptr)` | 释放结果/错误/元数据指针（task 结果同时移除 task；每个指针只调一次） |
| `memory` | 线性内存导出 |

`detail`/`watch` 与 `latest`/`search` 共用同一条异步桥：返回 guest handle，
宿主 pump 轮询 `take_result`，错误经 `last_error` 上报。

guest handle 形如 `0x40000000 | slab_index`，永不与 host handle（<0x40000000）混淆。

## 结果 JSON 契约（v0.2 新增）

`latest`/`search` 返回**条目数组**，条目形状由插件自定义（本仓库示例插件为
`{"title","url","cover","update"}`）。`detail`/`watch` 返回**单个对象**：

### detail(url) -> object

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `title` | string | 作品标题 |
| `cover` | string | 封面 URL（无封面时为空串） |
| `description` | string | 简介（无简介时为空串） |
| `episodes` | array | 分集列表，元素为 `{"title":string,"url":string}` |

`episodes[].url` 是**插件私有协议值**，供调用方原样回传给同一插件的
`watch`；宿主不得解析其结构。`detail` 的入参 `url` 通常是 `latest`/`search`
返回条目的 `url` 字段。

### watch(url) -> object

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `url` | string | 可直接请求的媒体地址（HTTP 直链） |
| `headers` | object? | 可选请求头（如 `Referer`/`User-Agent`）；省略表示无需额外头 |

### 示例插件约定

- **360zy**：`detail` 的 `episode.url` 取自 maccms `vod_play_url` 的直链；
  `watch` 原样返回 `{"url": url}`（maccms 提供直链，无需二次解析）。
- **bili-wbi**：`episode.url` 编码为 `{bvid}|{cid}|{page}`（竖线分隔的
  **私有协议**）。`watch` 解析后调用 WBI 签名的 playurl：`fnval=16` 优先取
  `data.dash.video` 中最高 `bandwidth` 的 `baseUrl`，失败回退
  `data.durl[0].url`；返回对象带 `Referer: https://www.bilibili.com` 与
  `User-Agent` 请求头。

## Host import（module "grain"）

| import | 响应 JSON |
| --- | --- |
| `grain_log(level, ptr, len)` | —；level 0=debug 1=info 2=warn 3=error |
| `grain_http_request(req_ptr, req_len) -> handle` | req `{"url","method","headers":{...},"body"}`；resp `{"status":int,"headers":{...},"body":"..."}` 或错误信封 |
| `grain_xpath_query(html_ptr,html_len,xpath_ptr,xpath_len) -> handle` | `{"results":["text",...]}` 或错误信封 |
| `grain_crypto_md5(ptr, len) -> handle` | `{"value":"hex"}` 或错误信封 |
| `grain_crypto_base64_encode(ptr, len) -> handle` | `{"value":"b64"}` 或错误信封 |
| `grain_event_wait(handle, buf_ptr, buf_cap) -> i32` | 0=未完成；-1=buf 太小（guest 翻倍重试）；-2=未知 handle；>0=已写入 buf 的响应 JSON 字节数 |

http 响应的 `headers` 必须含原始 `Date` 头——wasm 内无时钟，插件靠它推导时间。

## 错误信封（v0.1 新增）

错误统一为 `{"code":string,"message":string,"detail"?:string,"httpStatus"?:int}`：

| code | 语义 | 宿主侧异常 |
| --- | --- | --- |
| `plugin` | 插件自身逻辑/业务错误（guest `Error::msg`） | `PluginLogicError` |
| `http` | 宿主能力层失败（网络、源站非 2xx 语义）；`httpStatus` 带上源站状态码 | `CapabilityError(httpStatus)` |
| `abi` | 协议违例：缺导出、import 形状不符、非法请求、结果缓冲耗尽 | `AbiError` |
| `fuel` | 沙箱/资源：fuel 耗尽、trap、宿主结果表打满、主机白名单拒绝 | `SandboxError` |
| `timeout` | 宿主泵超时（pump 默认预算 30s，`callLatest/callSearch/callDetail/callWatch(timeout:)`） | `TimeoutError` |

- guest PDK `Error` 带 `code` 字段；`last_error()` 输出本信封。
- 宿主能力层（`_runHttp` 等）失败时把带 code 的信封透传给插件，PDK
  `parse_host_response` 归一化保留 code。
- 宿主 pump 解析信封按上表类型化抛出；**旧式 v0 信封 `{"error":"..."}`
  仍被容忍**：guest 侧按 `plugin` 处理，宿主侧回退 `PluginException`。
- 宿主另设每插件日志环（上限 500 条）：guest `grain_log` 与该插件的
  宿主侧事件（http 异常、event_wait 错误、fuel/trap）进同一环，
  `GrainPlugin.logs` 读取；日志接收器 `GrainLogger` 可在 instantiate 注入。

## v0 → v0.1 差异

1. `grain_log` 签名 `(msg_ptr, msg_len)` → `(level:i32, msg_ptr, msg_len)`。
2. 错误通道 `{"error":"..."}` → 结构化错误信封（上节）。
3. **兼容性**：v0 插件必须**用 v0.1 PDK 重编译**——`grain_log` import
    arity 变更使旧 wasm 无法实例化（instantiate 会报 import 不匹配）；
   错误信封双向容忍只是过渡期的解析兜底。

## v0.1 → v0.2 差异

**纯新增，无破坏性变更。**

1. guest 新增可选导出 `detail(url)` 与 `watch(url)`；宿主新增
   `callDetail(url)` / `callWatch(url)`（与 `callLatest`/`callSearch` 同款泵）。
2. 导出清单 9 → 11。未实现这两个导出的插件在宿主调用时抛 `AbiError`
   （与缺失 `search` 同款文案），不影响其他调用。
3. **兼容性**：v0.1 插件**无需重编译**即可继续运行；需要详情/播放能力的插件
   按需实现两个导出即可。

参考实现：`pdk/rust/tools/mock_host.mjs`（Node，含异步时延模拟、错误通道与
detail/watch 路由演示）。

## 异步桥（核心结论）

wasm_run 的 import 是纯同步 Dart 闭包，guest 调用栈无法挂起，wasm_run 无
async import。落地架构（guest PDK 的 async/await 状态机 + 宿主泵）：

1. 宿主调 `latest(page)` → `#[plugin_fn]` 生成的导出函数把插件 async fn 装进
   task slab，**同步做完所有能做的**，立即返回 handle。
2. 状态机本体由 Rust async/await 编译器生成（插件作者无感）。
3. 宿主轮询 `take_result(handle)`：每次调用 poll 一次 future（noop waker）。
   - `Pending` → 返回 0；宿主 await 自己的 dio Future 后再次轮询。
   - `Ready(Ok(json))` → 结果写进 NUL 结尾缓冲，返回指针。
   - `Ready(Err)` → 置 `last_error` 并返回 0（出错终态，重轮询会重复上报）。
4. future 内部 `await grain_pdk::http(...)`：第一次 poll 调
   `grain_http_request` import 拿 host handle 后 Pending；之后每次 poll 调
   `grain_event_wait(handle, buf, cap)` 同步检查——宿主 Future 已完成则把响应
   JSON 写进 guest 内存返回字节数，future 当场继续推进。

限制：guest 不得在任何导出函数里自旋等 host（卡死事件循环且烧 fuel）；
异步边界只能走「handle + event_wait 直写 + 宿主泵」。

## 元数据

`grain_pdk::plugin_meta!{ "name": "...", ... }` 展开为：
- 常量 JSON（编译期 `concat!` 拼接）
- `#[used] #[link_section = "grain.metadata"]` 静态字节 → wasm custom section
  （宿主从原始 wasm 字节解析；guest 编译期嵌入，无需 build.rs/wasm-opt）
- ABI 导出 `metadata_json()`（NUL 结尾缓冲，每次调用新分配）

## ABI 强制校验规则

宿主 `instantiate` 对「真正使用 grain 能力」的插件（声明了 grain import 或
带有 `metadata_json`/`latest`/`search`/`detail`/`watch`/`take_result`/`last_error`
导出）强制要求 `alloc`/`dealloc`/`take_result`/`free_result` 四个导出；
`detail`/`watch` 本身是可选导出，不要求存在。裸探针模块
（如 panic_test 只有 spin/panic/add）放行。

## 性能数据（六项验证，2026-11-19 实测）

| # | 验证项 | 结果 |
|---|--------|------|
| 1 | latest 耗时/条数 | 969 ms / 20 条（真实 360资源数据；冷启动首轮 ~12s 为 7 域名串行故障转移，稳态 ~1s） |
| 2 | 异步桥（http/xpath/crypto） | search("孤独摇滚") 436 ms 精确命中，dio→event_wait 直写链路 |
| 3 | fuel 掐死死循环 | spin() 0~5 ms 内 trap：`all fuel consumed by WebAssembly`（100000 fuel 耗尽） |
| 4 | panic 恢复 | trap（`wasm unreachable`）被宿主捕获，之后同进程新实例正常装载 |
| 5 | 内存曲线 | RSS 1→73.8 MB，5→87.8，10→98.0，20→118.6 MB，约 2~2.6 MB/插件 |
| 6 | WBI 签名全流程 | latest（免签名 /popular）176 ms / 20 条真实数据；search 被源站风控拦截（机房 IP），签名链经 mock 宿主 + Python 参考向量验证 |

harness 在六项之后另有 [7]/[8] 两项 detail/watch 判决（360zy 详情 → 首集
直链 → HTTP 200/206 校验），其结果取决于运行环境的真实网络。

## 宿主环境踩坑记录

- **wasm_run 原生库不在 pub 包**：首次使用需 `dart run wasm_run:setup` 从
  github releases 拉取（约 20MB），网络受限环境需重试或断点续传。
- **Windows GUI runner 不透传 argv**：`Platform.executableArguments` 拿不到
  启动参数，harness 用 `WASM_DIR` 环境变量覆盖插件目录。
- **Rust extern import 默认进 `env` module**：必须
  `#[link(wasm_import_module = "grain")]` 才能对上宿主注册的 module "grain"
  （PDK 已处理，插件作者无感）。
- **wasm 内存增长会 detach 宿主侧 DataView**：宿主每次访问线性内存都要新建视图。
- **import 回调里抛异常会直接 segfault 整个进程**（wasm_run 实测）：宿主侧
  event_wait 把异常兜成 -2 未知 handle，错误细节走宿主日志。
- **dio 把响应 header 名规范为小写**（`date`），guest 按 ABI 约定大小写敏感查
  `Date`——宿主在 headerMap 补规范大小写键。

## guest 侧踩坑记录

1. **无 MSVC**：proc-macro 等宿主构件需要链接器；用 rustc 自带
   `rust-lld.exe`（`.cargo/config.toml` 已配置），无需安装 VS Build Tools。
2. **`concat!` 吃字面量内容不给引号**：`plugin_meta!` 的键要自己包 `\"`；
   而 `stringify!` 保留字面量源码文本（自带引号）——两者行为相反，别混。
3. **宏插值 `#pt.ty`** 会被解析成插值 `pt` + 字面 `.ty`，要先 `let ty = &pt.ty`。
4. **复用的 ABI 堆块不保证零填充**：宿主往 `alloc` 返回的内存写字符串必须
   显式写 NUL 结尾。
5. **wasm 内无时钟**：需要时间参数（如按更新小时过滤、WBI 的 wts）只能从
   响应 `Date` 头推导；ABI v1 若加 `grain_now()` 可省一次请求。
