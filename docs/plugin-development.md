# grain_engine 插件开发指南

## 快速开始（Rust）

前置：Rust stable + target `wasm32-unknown-unknown`。

```bash
rustup target add wasm32-unknown-unknown
cd pdk/rust
cargo build --target wasm32-unknown-unknown --release
# 产物：target/wasm32-unknown-unknown/release/*.wasm
```

### 最小插件

```rust
use grain_pdk::{plugin_fn, plugin_meta, Result};
use serde_json::{json, Value};

plugin_meta! {
    "name": "我的插件",
    "version": "v0.1.0",
    "type": "bangumi",
    "package": "example.com",
    "author": "me",
    "lang": "zh-cn",
    "webSite": "https://example.com",
    "nsfw": false,
}

#[plugin_fn]
pub async fn latest(page: u32) -> Result<Value> {
    let resp = grain_pdk::http(
        "GET",
        &format!("https://example.com/api?page={}", page),
        "{}",
        "",
    ).await?;
    let v = resp.json()?;
    Ok(json!([]))
}

#[plugin_fn]
pub async fn search(keyword: String) -> Result<Value> {
    // keyword 是宿主写入的 NUL 结尾 UTF-8 串，宏自动解码
    Ok(json!([]))
}
```

`#[plugin_fn]` 把 `async fn` 转成 ABI 导出：函数体装进 task slab，
返回 guest handle，异步状态机由编译器生成，**插件作者完全无感**。

参数映射：`u32`/`i32` ← i32；`u64`/`i64` ← i64；`String` ← kw_ptr（NUL 结尾串）。
返回类型必须是 `Result<T, E>`（`T: serde::Serialize`，`E: Display`）。

### 宿主能力（全部异步，沙箱内 await 即可）

| PDK API | 说明 |
| --- | --- |
| `grain_pdk::http(method, url, headers_json, body)` | 宿主代发 HTTP；响应含 `status`/`headers`/`body` |
| `grain_pdk::xpath_query(html, xpath)` | 宿主 XPath 查询，返回文本数组 |
| `grain_pdk::host_md5(data)` / `host_base64(data)` | 宿主加密（插件内也有纯 Rust `md5_hex`/`base64_encode` 可对照） |
| `grain_pdk::log(level, msg)` | 宿主日志（`LogLevel::Debug/Info/Warn/Error`，进宿主每插件日志环） |
| `grain_pdk::urlencode(s)` / `md5_hex(b)` / `base64_encode(b)` | 纯 Rust 计算原语，不依赖宿主 |
| `grain_pdk::http_date_hour_utc(s)` / `http_date_to_unix(s)` | HTTP Date 头解析（wasm 内唯一时间来源） |

要点：
- **wasm 内无时钟**：需要时间就从 HTTP 响应 `Date` 头推导（见上表）。
- **不要自旋等宿主**：任何导出函数里死循环等结果会卡死事件循环并烧光 fuel；
  异步边界只能 await PDK 的宿主操作。
- 纯计算（签名、编码、解析）尽量在插件内做（纯 Rust 原语），只有 IO 走宿主。

### 验证插件

```bash
# 单元测试（宿主，rust-lld 链接）
cargo test -p grain-pdk -p plugin-bili-wbi

# Node mock 宿主端到端（无需真实网络）
node tools/mock_host.mjs target/wasm32-unknown-unknown/release/plugin_360zy.wasm latest
node tools/mock_host.mjs target/wasm32-unknown-unknown/release/plugin_bili_wbi.wasm search "鬼灭"

# 检查 wasm 二进制（custom section/import/export）
python tools/inspect_wasm.py target/wasm32-unknown-unknown/release/plugin_360zy.wasm
```

### 在宿主（Dart/Flutter）中装载

```dart
final plugin = await GrainPlugin.instantiate('plugin_360zy.wasm');
final latest = await plugin.callLatest(1);
final search = await plugin.callSearch('孤独摇滚');
print(plugin.metadata); // custom section grain.metadata
plugin.dispose();
```

资源控制：
- `GrainPlugin.instantiate(path, consumeFuel: true, fuelAmount: 100000)`
  给插件加 fuel 上限，死循环会在耗尽后被 trap 掐死。
- trap（guest panic/unreachable）只影响该实例，宿主进程与其他插件不受影响。

完整示例见 `example/harness/`（`flutter run -d windows --release`，
`WASM_DIR` 环境变量指向 wasm 产物目录）。

## 其他语言（规划）

ABI 与语言无关：任何能编译到 `wasm32-unknown-unknown` 的语言都可以写插件，
只需实现 [ABI v0.2](abi.md) 的 11 个导出（其中 `detail`/`watch` 可选）并
import module "grain" 的 6 个函数。

- **AssemblyScript**：与 JS 心智最接近，适合前端开发者；规划提供
  as-grain-pdk 绑定层（尚无排期）。
- **TinyGo / C / Zig**：同样可行，遵循 ABI 文档手工实现导出即可。

非 Rust 语言插件在落地前的临时方案：用 Rust PDK 写薄包装，内部通过
stdio 协议桥接外部进程（会破坏单沙箱模型，不推荐）。
