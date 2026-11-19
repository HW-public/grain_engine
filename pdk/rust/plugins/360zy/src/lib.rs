//! 360资源（360zy.com）API 型插件 —— Rust 复刻 JS 基线扩展的 latest/search。
//! JS 参考：`js_engine/assets/extensions/360zy.com.js`（hualiong, v0.0.2）。

use grain_pdk::{plugin_fn, plugin_meta, Error, HttpResponse, Result};
use serde_json::{json, Value};

plugin_meta! {
    "name": "360资源",
    "version": "v0.0.2",
    "type": "bangumi",
    "package": "360zy.com",
    "author": "hualiong",
    "lang": "zh-cn",
    "webSite": "https://360zy.com",
    "nsfw": false,
}

/// JS 基线版用自定义头做多域名并发竞速，WASM 宿主里没有对应物，这里串行故障转移。
const DOMAINS: [&str; 7] = [
    "360zy.com",
    "360zy.net",
    "360zy.top",
    "360zy.tv",
    "360zy1.com",
    "360zy2.com",
    "360zy3.com",
];

thread_local! {
    /// maccms `h` 参数（按更新小时过滤，JS 版恒带）。
    /// wasm 内无时钟，从 HTTP 响应 Date 头推导（东九区小时，0 归一为 24），缓存复用。
    static CACHED_H: ::std::cell::Cell<i32> = const { ::std::cell::Cell::new(-1) };
}

async fn fetch_vod(params: &str) -> Result<HttpResponse> {
    let mut last_err = String::from("no domain attempted");
    for domain in DOMAINS {
        let url = format!("https://{}/api.php/provide/vod?ac=detail{}", domain, params);
        match grain_pdk::http("GET", &url, "{}", "").await {
            Ok(resp) if resp.status == 200 && resp.body.trim_start().starts_with('{') => {
                return Ok(resp);
            }
            Ok(resp) => last_err = format!("{}: HTTP {}", domain, resp.status),
            Err(e) => last_err = format!("{}: {}", domain, e),
        }
        grain_pdk::log(
            grain_pdk::LogLevel::Warn,
            &format!("[360zy] {} failed: {}", domain, last_err),
        );
    }
    Err(Error::msg(format!(
        "all 360zy domains failed, last: {}",
        last_err
    )))
}

fn maybe_cache_h(resp: &HttpResponse) {
    if let Some(date) = resp.header("Date") {
        if let Some(h) = grain_pdk::http_date_hour_utc(date) {
            let h = ((h + 9) % 24) as i32;
            CACHED_H.with(|c| c.set(if h == 0 { 24 } else { h }));
        }
    }
}

fn map_list(v: Value) -> Value {
    let empty = Vec::new();
    let list = v.get("list").and_then(|x| x.as_array()).unwrap_or(&empty);
    let items: Vec<Value> = list
        .iter()
        .filter_map(|e| {
            let title = e.get("vod_name")?.as_str()?;
            let id = match e.get("vod_id") {
                Some(Value::Number(n)) => n.to_string(),
                Some(Value::String(s)) => s.clone(),
                _ => return None,
            };
            Some(json!({
                "title": title,
                "url": id,
                "cover": e.get("vod_pic").and_then(|x| x.as_str()).unwrap_or(""),
                "update": e.get("vod_remarks").and_then(|x| x.as_str()).unwrap_or(""),
            }))
        })
        .collect();
    Value::Array(items)
}

async fn do_latest(page: u32) -> Result<Value> {
    let params = match CACHED_H.with(|c| c.get()) {
        h if h > 0 => format!("&pg={}&h={}", page, h),
        _ => format!("&pg={}", page),
    };
    let resp = fetch_vod(&params).await?;
    maybe_cache_h(&resp);
    Ok(map_list(resp.json()?))
}

async fn do_search(kw: &str) -> Result<Value> {
    if kw.trim().is_empty() {
        return do_latest(1).await;
    }
    // ABI v0 的 search 只有 keyword 一个参数，页码固定 1（JS 版的 filter 也一并省略）
    let params = format!("&wd={}&pg=1", grain_pdk::urlencode(kw));
    let resp = fetch_vod(&params).await?;
    Ok(map_list(resp.json()?))
}

#[plugin_fn]
pub async fn latest(page: u32) -> Result<Value> {
    do_latest(page).await
}

#[plugin_fn]
pub async fn search(keyword: String) -> Result<Value> {
    do_search(&keyword).await
}
