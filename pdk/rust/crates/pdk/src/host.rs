//! 宿主 import（module "grain"）的异步封装。
//!
//! 宿主 import 全是同步闭包：HostOp future 第一次 poll 发起 import 拿 host handle，
//! 之后每次 poll 调 `grain_event_wait(handle, buf, cap)` 同步取结果——宿主 Future 已
//! 完成则把响应 JSON 写进 guest 内存并返回字节数，future 当场推进；未完成返回 0
//! （future 保持 Pending，等宿主下一轮 take_result 轮询）；buf 太小 -1（换大 buffer
//! 重试）；未知 handle -2。guest 从不自旋等待。

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::heap;
use crate::Error;

#[cfg(target_arch = "wasm32")]
mod imp {
    #[link(wasm_import_module = "grain")]
    extern "C" {
        /// (level, ptr, len) 裸 UTF-8 字节日志；level 0=debug 1=info 2=warn 3=error
        pub fn grain_log(level: i32, msg_ptr: i32, msg_len: i32);
        /// 请求 JSON {"url","method","headers":{...},"body":"..."}（ptr,len）；返回 host handle
        pub fn grain_http_request(req_ptr: i32, req_len: i32) -> i32;
        /// 返回 host handle；响应 {"results":[...]} 或错误信封
        pub fn grain_xpath_query(
            html_ptr: i32,
            html_len: i32,
            xpath_ptr: i32,
            xpath_len: i32,
        ) -> i32;
        /// 返回 host handle；响应 {"value":"hex"} 或错误信封
        pub fn grain_crypto_md5(data_ptr: i32, data_len: i32) -> i32;
        /// 返回 host handle；响应 {"value":"b64"} 或错误信封
        pub fn grain_crypto_base64_encode(data_ptr: i32, data_len: i32) -> i32;
        /// 0=未完成；-1=buf 太小；-2=未知 handle；>0=已把响应 JSON 写入 buf 的字节数
        pub fn grain_event_wait(handle: i32, buf_ptr: i32, buf_cap: i32) -> i32;
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    // 非 wasm 目标（cargo test）的链接桩：测试不会走到这里
    pub unsafe fn grain_log(_: i32, _: i32, _: i32) {
        panic!("wasm-only host import")
    }
    pub unsafe fn grain_http_request(_: i32, _: i32) -> i32 {
        panic!("wasm-only host import")
    }
    pub unsafe fn grain_xpath_query(_: i32, _: i32, _: i32, _: i32) -> i32 {
        panic!("wasm-only host import")
    }
    pub unsafe fn grain_crypto_md5(_: i32, _: i32) -> i32 {
        panic!("wasm-only host import")
    }
    pub unsafe fn grain_crypto_base64_encode(_: i32, _: i32) -> i32 {
        panic!("wasm-only host import")
    }
    pub unsafe fn grain_event_wait(_: i32, _: i32, _: i32) -> i32 {
        panic!("wasm-only host import")
    }
}

/// 日志级别（ABI grain_log 的 level 参数）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
}

pub fn log(level: LogLevel, msg: &str) {
    unsafe {
        with_arg_bytes(msg.as_bytes(), |p, l| imp::grain_log(level as i32, p, l));
    }
}

/// 把入参字节临时放进 ABI 堆，调 import，然后释放。
/// 分配失败时以 (0, 0) 调用宿主（宿主约定为空串）。
unsafe fn with_arg_bytes<R>(bytes: &[u8], f: impl FnOnce(i32, i32) -> R) -> R {
    let len = bytes.len().max(1);
    let p = heap::alloc_bytes(len);
    if p.is_null() {
        return f(0, 0);
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
    let r = f(p as i32, bytes.len() as i32);
    heap::dealloc(p, len);
    r
}

/// 首次 event_wait 用的接收缓冲；遇到 -1 翻倍。
const EVENT_BUF_CAP: usize = 64 * 1024;

/// 宿主操作 Future：第一次 poll 发起 import 拿 host handle，之后每次 poll
/// 用 event_wait 同步取结果。所有进度都由 take_result 轮询驱动，waker 是摆设。
pub struct HostOp {
    handle: i32, // <0 = 发起失败
    start_error: Option<String>,
}

impl HostOp {
    fn new(handle: i32) -> Self {
        HostOp {
            handle,
            start_error: None,
        }
    }
}

impl Future for HostOp {
    type Output = crate::Result<String>;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if let Some(e) = this.start_error.take() {
            return Poll::Ready(Err(Error::msg(e)));
        }
        if this.handle < 0 {
            return Poll::Ready(Err(Error::msg("host operation failed to start")));
        }
        let mut cap = EVENT_BUF_CAP;
        loop {
            let buf = unsafe { heap::alloc_bytes(cap) };
            if buf.is_null() {
                return Poll::Ready(Err(Error::with_code("abi", "ABI heap exhausted")));
            }
            let n = unsafe { imp::grain_event_wait(this.handle, buf as i32, cap as i32) };
            if n > 0 {
                let bytes = unsafe { std::slice::from_raw_parts(buf, n as usize) };
                let s = String::from_utf8_lossy(bytes).into_owned();
                unsafe { heap::dealloc(buf, cap) };
                return Poll::Ready(Ok(s));
            }
            unsafe { heap::dealloc(buf, cap) };
            match n {
                0 => return Poll::Pending,
                -1 => {
                    cap *= 2;
                    continue;
                }
                -2 => {
                    return Poll::Ready(Err(Error::msg(format!(
                        "unknown host handle {}",
                        this.handle
                    ))))
                }
                other => {
                    return Poll::Ready(Err(Error::msg(format!(
                        "grain_event_wait returned {}",
                        other
                    ))))
                }
            }
        }
    }
}

/// 解析宿主响应（错误信封 → Err，code 透传；旧式 v0 {"error":"..."} 容忍）。
/// 成功返回原文。
fn parse_host_response(raw: &str) -> crate::Result<String> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(e) = v.get("error").and_then(|x| x.as_str()) {
            // 旧式 v0 信封：无 code，按插件错误处理。
            return Err(Error::msg(e));
        }
        if let Some(message) = v.get("message").and_then(|x| x.as_str()) {
            if let Some(code) = v.get("code").and_then(|x| x.as_str()) {
                // 宿主能力层错误信封：code 归一化为静态判别码（"http"/"abi"/"fuel"/"timeout"）。
                return Err(Error::with_code(
                    match code {
                        "http" => "http",
                        "abi" => "abi",
                        "fuel" => "fuel",
                        "timeout" => "timeout",
                        _ => "plugin",
                    },
                    message,
                ));
            }
        }
    }
    Ok(raw.to_string())
}

/// HTTP 响应（宿主信封 {"status":int,"headers":{...},"body":"..."} 的解包）。
pub struct HttpResponse {
    pub status: i64,
    pub body: String,
    pub headers: serde_json::Map<String, serde_json::Value>,
}

impl HttpResponse {
    /// 按 JSON 解析 body。
    pub fn json(&self) -> crate::Result<serde_json::Value> {
        serde_json::from_str(&self.body)
            .map_err(|e| Error::msg(format!("invalid JSON body: {}", e)))
    }

    /// 取响应头（大小写敏感，宿主应原样透传）。
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.as_str())
    }
}

/// 发起 HTTP 请求。`headers_json` 是 JSON 对象字符串（如 "{}"/{"User-Agent":"..."}）。
pub async fn http(
    method: &str,
    url: &str,
    headers_json: &str,
    body: &str,
) -> crate::Result<HttpResponse> {
    let headers: serde_json::Value =
        serde_json::from_str(headers_json).unwrap_or_else(|_| serde_json::json!({}));
    let req = serde_json::json!({
        "url": url,
        "method": method,
        "headers": headers,
        "body": body,
    })
    .to_string();
    let raw = HostOp::new(unsafe {
        with_arg_bytes(req.as_bytes(), |p, l| imp::grain_http_request(p, l))
    })
    .await?;
    let raw = parse_host_response(&raw)?;
    let v: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| Error::with_code("abi", format!("bad http result envelope: {}", e)))?;
    let status = v.get("status").and_then(|x| x.as_i64()).unwrap_or(0);
    let body = v
        .get("body")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let headers = v
        .get("headers")
        .and_then(|x| x.as_object())
        .cloned()
        .unwrap_or_default();
    Ok(HttpResponse {
        status,
        body,
        headers,
    })
}

/// XPath 查询，结果 = 文本数组。
pub async fn xpath_query(html: &str, xpath: &str) -> crate::Result<Vec<String>> {
    let raw = HostOp::new(unsafe {
        with_arg_bytes(html.as_bytes(), |hp, hl| {
            with_arg_bytes(xpath.as_bytes(), |xp, xl| {
                imp::grain_xpath_query(hp, hl, xp, xl)
            })
        })
    })
    .await?;
    let raw = parse_host_response(&raw)?;
    let v: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| Error::with_code("abi", format!("bad xpath result: {}", e)))?;
    let arr = v
        .get("results")
        .and_then(|x| x.as_array())
        .ok_or_else(|| Error::with_code("abi", "xpath result missing results array"))?;
    Ok(arr
        .iter()
        .filter_map(|x| x.as_str().map(|s| s.to_string()))
        .collect())
}

/// 宿主 MD5（插件内已有纯 Rust 实现 hash::md5_hex 可对照）。
pub async fn host_md5(data: &str) -> crate::Result<String> {
    let raw =
        HostOp::new(unsafe { with_arg_bytes(data.as_bytes(), |p, l| imp::grain_crypto_md5(p, l)) })
            .await?;
    let raw = parse_host_response(&raw)?;
    let v: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| Error::with_code("abi", format!("bad md5 result: {}", e)))?;
    v.get("value")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| Error::with_code("abi", "md5 result missing value"))
}

/// 宿主 Base64 编码（插件内已有 hash::base64_encode 可对照）。
pub async fn host_base64(data: &str) -> crate::Result<String> {
    let raw = HostOp::new(unsafe {
        with_arg_bytes(data.as_bytes(), |p, l| {
            imp::grain_crypto_base64_encode(p, l)
        })
    })
    .await?;
    let raw = parse_host_response(&raw)?;
    let v: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| Error::with_code("abi", format!("bad base64 result: {}", e)))?;
    v.get("value")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| Error::with_code("abi", "base64 result missing value"))
}
