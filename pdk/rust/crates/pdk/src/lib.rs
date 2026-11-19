//! grain-pdk：grain 插件 ABI v0.1 的 WASM guest 端 PDK。
//!
//! # 异步模型（与宿主约定，见仓库根 docs/abi.md）
//! wasm import 是同步闭包，guest 调用栈无法挂起，因此：
//! 1. 导出入口（`latest`/`search`）把插件的 async fn 装进 task slab，
//!    同步做完所有能做的（包括发起 `grain_http_request` 拿 host handle），
//!    立即返回 guest handle（0x4000_0000 | index；0 保留为立即失败）。
//! 2. 状态机本体由 Rust async/await 编译器生成，插件作者无感。
//! 3. 宿主轮询 `take_result(handle)`：每次调用 poll 一次 future。
//!    - Pending → 返回 0（未完成）；宿主 await 自己的 dio Future 后再来。
//!    - future 内部 await 宿主操作时，poll 调 `grain_event_wait(handle, buf, cap)`
//!      同步检查：宿主 Future 已完成则把响应 JSON 写进 buf 返回字节数（>0），
//!      future 当场继续推进；未完成返回 0；buf 太小 -1（换大 buffer 重试）；
//!      未知 handle -2。
//!    - Ready(Ok) → 结果写进 NUL 结尾缓冲返回指针；宿主读后经 `free_result(ptr)` 释放。
//!    - Ready(Err) → 置 last_error 并返回 0；宿主看到 take_result==0 时
//!      应查 `last_error()` 区分「未完成」与「出错」。
//!
//! # 字符串协议（ABI v0 最终版）
//! 出参全部是 NUL 结尾 UTF-8 缓冲：`[bytes...][0x00]`。
//! 入参（宿主写入）：`alloc(len)` 拿内存，写裸字节 + NUL，传 ptr
//! （如 `search(kw_ptr)`；带 len 的 import 如 `grain_log(ptr,len)` 例外）。

mod hash;
mod heap;
mod host;

pub use grain_pdk_macros::plugin_fn;
pub use hash::{base64_encode, http_date_hour_utc, http_date_to_unix, md5_hex, urlencode};
pub use host::{host_base64, host_md5, http, log, xpath_query, HttpResponse, LogLevel};

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

pub type Result<T> = std::result::Result<T, Error>;

/// 插件错误（ABI v0.1 错误信封的 guest 侧载体）。
///
/// [code] 是信封判别码（`plugin`/`http`/`abi`/`fuel`/`timeout`），宿主
/// pump 据类型化抛出；宿主能力层透传的错误信封（带 code）经
/// `host::parse_host_response` 保留 code。
#[derive(Debug, Clone)]
pub struct Error {
    pub code: &'static str,
    pub message: String,
}

impl Error {
    /// 插件自身错误（code = `plugin`）。
    pub fn msg(m: impl Into<String>) -> Error {
        Error {
            code: "plugin",
            message: m.into(),
        }
    }

    /// 指定错误码（透传宿主信封 / PDK 内部资源错误用）。
    pub fn with_code(code: &'static str, m: impl Into<String>) -> Error {
        Error {
            code,
            message: m.into(),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl From<String> for Error {
    fn from(m: String) -> Error {
        Error::msg(m)
    }
}

impl From<&str> for Error {
    fn from(m: &str) -> Error {
        Error::msg(m)
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Error {
        Error::msg(format!("json: {}", e))
    }
}

// ---------------------------------------------------------------------------
// ABI 字符串缓冲区（NUL 结尾）
// ---------------------------------------------------------------------------

/// 分配 NUL 结尾 ABI 字符串缓冲 `[bytes...][0x00]` 并返回指针。失败返回 0。
pub fn alloc_abi_bytes(bytes: &[u8]) -> i32 {
    let total = bytes.len() + 1;
    let p = unsafe { heap::alloc_bytes(total) };
    if p.is_null() {
        return 0;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        *p.add(bytes.len()) = 0;
    }
    p as i32
}

/// 读 NUL 结尾 ABI 字符串并释放缓冲。
///
/// # Safety
/// `ptr` 必须是本 crate ABI 堆分配、NUL 结尾的字符串指针（`alloc_abi_bytes` 的返回值），
/// 且只能被 take 一次；重复 take 同一指针是未定义行为。
pub unsafe fn take_abi_string(ptr: i32) -> String {
    if ptr <= 0 {
        return String::new();
    }
    let p = ptr as *mut u8;
    let mut len = 0usize;
    while *p.add(len) != 0 {
        len += 1;
    }
    let bytes = std::slice::from_raw_parts(p, len);
    let s = String::from_utf8_lossy(bytes).into_owned();
    heap::dealloc(p, len + 1);
    s
}

/// 宿主写入的 NUL 结尾字符串（只读拷贝，不释放）。
///
/// # Safety
/// `ptr` 必须指向一块以 NUL 结尾、当前存活且可读的内存（宿主经 event_wait 直写的结果缓冲）。
pub unsafe fn read_cstr(ptr: i32) -> String {
    if ptr <= 0 {
        return String::new();
    }
    let p = ptr as *const u8;
    let mut len = 0usize;
    while *p.add(len) != 0 {
        len += 1;
    }
    String::from_utf8_lossy(std::slice::from_raw_parts(p, len)).into_owned()
}

pub fn to_json_string<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".to_string())
}

// ---------------------------------------------------------------------------
// ABI 导出：alloc / dealloc
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn alloc(len: i32) -> i32 {
    unsafe { heap::alloc_bytes(len.max(0) as usize) as i32 }
}

#[no_mangle]
pub extern "C" fn dealloc(ptr: i32, len: i32) {
    if ptr > 0 {
        unsafe { heap::dealloc(ptr as *mut u8, len.max(0) as usize) };
    }
}

// ---------------------------------------------------------------------------
// task slab 与导出：take_result / free_result / last_error
// ---------------------------------------------------------------------------

/// guest task handle 带 0x4000_0000 前缀，以便与 host handle 区分。
/// 约定：宿主返回的 host handle 必须 < 0x4000_0000。
const HANDLE_BASE: i32 = 0x4000_0000;
const HANDLE_MASK: i32 = 0x3fff_ffff;

/// plugin_fn 异步体的装箱 future 类型。
type BoxFuture = Pin<Box<dyn Future<Output = std::result::Result<String, Error>>>>;

struct Task {
    future: Option<BoxFuture>,
    /// 完成后缓存的 NUL 结尾结果缓冲指针，宿主 free_result(ptr) 前保持有效。
    result_ptr: i32,
    /// 出错终态：持久化错误（宿主反复轮询时重复上报 last_error）。
    err: Option<Error>,
}

thread_local! {
    static TASKS: RefCell<Vec<Option<Task>>> = const { RefCell::new(Vec::new()) };
}

/// 把插件 async fn 装进 task slab，返回 guest handle（0x4000_0000 | idx，永不为 0）。
pub fn spawn<F>(fut: F) -> i32
where
    F: Future<Output = std::result::Result<String, Error>> + 'static,
{
    TASKS.with(|t| {
        let mut tasks = t.borrow_mut();
        let task = Task {
            future: Some(Box::pin(fut)),
            result_ptr: 0,
            err: None,
        };
        if let Some((idx, slot)) = tasks.iter_mut().enumerate().find(|(_, s)| s.is_none()) {
            *slot = Some(task);
            HANDLE_BASE | idx as i32
        } else {
            tasks.push(Some(task));
            HANDLE_BASE | (tasks.len() as i32 - 1)
        }
    })
}

/// 无操作 waker：进度完全由宿主的 take_result 轮询驱动，唤醒只是重 poll 的信号。
static NOOP_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
    |_| RawWaker::new(std::ptr::null(), &NOOP_WAKER_VTABLE),
    |_| {},
    |_| {},
    |_| {},
);

fn noop_waker() -> Waker {
    unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &NOOP_WAKER_VTABLE)) }
}

enum Drive {
    Done(i32),
    Pending,
    Gone,
}

/// poll 一次 task future。不阻塞、不自旋——同步做完所有能做的就返回。
fn drive_task(handle: i32) -> i32 {
    let idx = (handle & HANDLE_MASK) as usize;
    let step = TASKS.with(|t| {
        let mut tasks = t.borrow_mut();
        match tasks.get_mut(idx) {
            Some(Some(task)) => {
                let fut = match task.future.as_mut() {
                    Some(f) => f,
                    None => {
                        // 终态：成功重放缓存指针；出错重复上报 last_error。
                        if let Some(e) = task.err.clone() {
                            set_last_error(&e);
                            return Drive::Done(0);
                        }
                        return Drive::Done(task.result_ptr);
                    }
                };
                let waker = noop_waker();
                let mut cx = Context::from_waker(&waker);
                match fut.as_mut().poll(&mut cx) {
                    Poll::Ready(Ok(s)) => {
                        task.future = None;
                        let p = alloc_abi_bytes(s.as_bytes());
                        if p == 0 {
                            let e =
                                Error::with_code("abi", "ABI heap exhausted while storing result");
                            set_last_error(&e);
                            task.err = Some(e);
                            Drive::Done(0)
                        } else {
                            task.result_ptr = p;
                            Drive::Done(p)
                        }
                    }
                    Poll::Ready(Err(e)) => {
                        task.future = None;
                        set_last_error(&e);
                        task.err = Some(e);
                        Drive::Done(0)
                    }
                    Poll::Pending => Drive::Pending,
                }
            }
            _ => Drive::Gone,
        }
    });
    match step {
        Drive::Done(p) => p,
        Drive::Pending | Drive::Gone => 0,
    }
}

/// ABI 导出：取结果。0 = 未完成（或出错——宿主应查 last_error 区分）；
/// 非 0 = NUL 结尾 JSON 指针，宿主读完调 free_result(ptr) 释放。
#[no_mangle]
pub extern "C" fn take_result(handle: i32) -> i32 {
    if handle <= 0 {
        return 0;
    }
    drive_task(handle)
}

/// ABI 导出：释放结果指针。task 结果 → 移除 task 并释放缓冲；
/// last_error / metadata_json 等一次性缓冲 → 直接按块释放（长度在块头里）。
/// 宿主对每个指针只调一次。
#[no_mangle]
pub extern "C" fn free_result(ptr: i32) {
    if ptr <= 0 {
        return;
    }
    TASKS.with(|t| {
        let mut tasks = t.borrow_mut();
        for slot in tasks.iter_mut() {
            if let Some(task) = slot {
                if task.result_ptr == ptr {
                    *slot = None;
                    break;
                }
            }
        }
    });
    unsafe { heap::dealloc(ptr as *mut u8, 0) };
}

// ---------------------------------------------------------------------------
// last_error（所有权转移：宿主读后自行 dealloc，槽位清零）
// ---------------------------------------------------------------------------

thread_local! {
    static LAST_ERROR: Cell<i32> = const { Cell::new(0) };
}

/// 置 last_error 为 ABI v0.1 错误信封 {"code":...,"message":...}。
fn set_last_error(e: &Error) {
    let json = format!(
        "{{\"code\":{},\"message\":{}}}",
        serde_json::to_string(e.code).unwrap(),
        serde_json::to_string(&e.message).unwrap()
    );
    let p = alloc_abi_bytes(json.as_bytes());
    LAST_ERROR.with(|c| {
        let old = c.get();
        if old > 0 {
            unsafe { take_abi_string(old) };
        }
        c.set(p);
    });
}

/// ABI 导出：取最近错误描述 JSON（ABI v0.1 错误信封）。无错误返回 0。
/// 返回后槽位清零，缓冲所有权归宿主（宿主 dealloc(ptr, 4+len)）。
#[no_mangle]
pub extern "C" fn last_error() -> i32 {
    LAST_ERROR.with(|c| c.replace(0))
}

// ---------------------------------------------------------------------------
// plugin_meta!：元数据进 custom section "grain.metadata"，并实现 metadata_json 导出
// ---------------------------------------------------------------------------

#[doc(hidden)]
#[macro_export]
macro_rules! __grain_meta_pairs {
    ($key:literal : $value:tt) => {
        concat!("\"", $key, "\":", stringify!($value))
    };
    ($key:literal : $value:tt, $($rest:tt)*) => {
        concat!("\"", $key, "\":", stringify!($value), ",", $crate::__grain_meta_pairs!($($rest)*))
    };
}

/// 声明插件元数据。键必须是 JSON 键字面量（带引号），值是 string/number/bool 字面量。
///
/// ```ignore
/// grain_pdk::plugin_meta! {
///     "name": "360资源",
///     "version": "v0.0.2",
///     "type": "bangumi",
///     "package": "360zy.com",
///     "author": "hualiong",
///     "nsfw": false,
/// }
/// ```
///
/// 展开物：常量 JSON 字符串、写进 custom section `grain.metadata` 的静态字节、
/// 以及 ABI 导出的 `metadata_json()`。
#[macro_export]
macro_rules! plugin_meta {
    ($($key:literal : $value:tt),+ $(,)?) => {
        pub const GRAIN_METADATA_JSON: &str =
            concat!("{", $crate::__grain_meta_pairs!($($key : $value),+), "}");

        const fn __grain_meta_bytes<const N: usize>(s: &str) -> [u8; N] {
            let mut out = [0u8; N];
            let src = s.as_bytes();
            let mut i = 0;
            while i < N {
                out[i] = src[i];
                i += 1;
            }
            out
        }

        #[used]
        #[link_section = "grain.metadata"]
        static GRAIN_METADATA_SECTION: [u8; GRAIN_METADATA_JSON.len()] =
            __grain_meta_bytes::<{ GRAIN_METADATA_JSON.len() }>(GRAIN_METADATA_JSON);

        /// ABI 导出：NUL 结尾 JSON 指针。每次调用新分配——宿主读完会调
        /// free_result(ptr) 释放，缓存会悬垂。
        #[no_mangle]
        pub extern "C" fn metadata_json() -> i32 {
            ::grain_pdk::alloc_abi_bytes(GRAIN_METADATA_JSON.as_bytes())
        }
    };
}
