//! 哔哩哔哩（WBI）搜索插件 —— 复杂校验专项。
//! 全流程在插件（WASM 沙箱）内完成：nav 取 img_key/sub_key → 按官方
//! mixin_key 置换表派生 key → 参数排序编码 → 插件内纯 Rust MD5 算 w_rid。
//! 时钟同样沙箱化：wts 取自 nav 响应的 Date 头（HTTP-date → Unix 秒）。

use grain_pdk::{plugin_fn, plugin_meta, Error, Result};
use serde_json::{json, Value};

plugin_meta! {
    "name": "哔哩哔哩(WBI)",
    "version": "v0.1.0",
    "type": "bangumi",
    "package": "bilibili.com.wbi",
    "author": "grain-engine",
    "lang": "zh-cn",
    "webSite": "https://www.bilibili.com",
    "nsfw": false,
}

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// 官方 mixin_key 置换表（64 项）。
const MIXIN_TAB: [usize; 64] = [
    46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49, 33, 9, 42, 19, 29,
    28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51, 30, 4, 22, 25,
    54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 34, 44, 52,
];

fn headers_json() -> String {
    json!({
        "User-Agent": UA,
        "Referer": "https://www.bilibili.com",
    })
    .to_string()
}

/// "https://i0.hdslb.com/bfs/wbi/<key>.png" → "<key>"（32 位十六进制）
fn key_from_url(url: &str) -> Option<String> {
    let seg = url.rsplit('/').next()?;
    let stem = seg.split('.').next()?;
    if stem.len() == 32 && stem.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(stem.to_string())
    } else {
        None
    }
}

fn derive_mixin_key(img_key: &str, sub_key: &str) -> String {
    let raw: Vec<char> = format!("{}{}", img_key, sub_key).chars().collect();
    MIXIN_TAB.iter().map(|&i| raw[i]).take(32).collect()
}

/// B 站 wbi 签名：补 wts → 按键名排序 → RFC3986 编码拼接 → md5(query + mixin_key)。
fn wbi_sign(mut params: Vec<(String, String)>, mixin_key: &str, wts: i64) -> String {
    params.push(("wts".to_string(), wts.to_string()));
    params.sort_by(|a, b| a.0.cmp(&b.0));
    let query = params
        .iter()
        .map(|(k, v)| format!("{}={}", grain_pdk::urlencode(k), grain_pdk::urlencode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let rid = grain_pdk::md5_hex(format!("{}{}", query, mixin_key).as_bytes());
    format!("{}&w_rid={}", query, rid)
}

struct WbiContext {
    mixin_key: String,
    wts: i64,
}

/// nav 接口：取 wbi 公钥对 + 用响应 Date 头推导 wts。
async fn fetch_wbi() -> Result<WbiContext> {
    let resp = grain_pdk::http(
        "GET",
        "https://api.bilibili.com/x/web-interface/nav",
        &headers_json(),
        "",
    )
    .await?;
    let date = resp
        .header("Date")
        .ok_or_else(|| Error::msg("nav response missing Date header"))?;
    let wts = grain_pdk::http_date_to_unix(date)
        .ok_or_else(|| Error::msg(format!("unparseable Date header: {}", date)))?;
    let v = resp.json()?;
    if v.get("code").and_then(|c| c.as_i64()) != Some(0) {
        let msg = v
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown");
        return Err(Error::msg(format!("nav error: {}", msg)));
    }
    let img = v
        .pointer("/data/wbi_img/img_url")
        .and_then(|x| x.as_str())
        .ok_or_else(|| Error::msg("nav: missing data.wbi_img.img_url"))?;
    let sub = v
        .pointer("/data/wbi_img/sub_url")
        .and_then(|x| x.as_str())
        .ok_or_else(|| Error::msg("nav: missing data.wbi_img.sub_url"))?;
    let img_key = key_from_url(img).ok_or_else(|| Error::msg(format!("bad img_url: {}", img)))?;
    let sub_key = key_from_url(sub).ok_or_else(|| Error::msg(format!("bad sub_url: {}", sub)))?;
    Ok(WbiContext {
        mixin_key: derive_mixin_key(&img_key, &sub_key),
        wts,
    })
}

/// B 站搜索标题带 <em class="keyword">高亮标签，剥掉。
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn map_video(bvid: &str, e: &Value) -> Value {
    json!({
        "title": e
            .get("title")
            .and_then(|x| x.as_str())
            .map(strip_tags)
            .unwrap_or_default(),
        "url": format!("https://www.bilibili.com/video/{}", bvid),
        "cover": match e.get("pic").and_then(|x| x.as_str()) {
            Some(p) if p.starts_with("//") => format!("https:{}", p),
            Some(p) => p.to_string(),
            None => String::new(),
        },
        "update": e
            .get("author")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

async fn do_search(keyword: &str) -> Result<Value> {
    if keyword.trim().is_empty() {
        return Err(Error::msg("keyword required"));
    }
    let wbi = fetch_wbi().await?;
    let params = vec![
        ("search_type".to_string(), "video".to_string()),
        ("keyword".to_string(), keyword.to_string()),
        ("page".to_string(), "1".to_string()),
    ];
    let query = wbi_sign(params, &wbi.mixin_key, wbi.wts);
    let url = format!(
        "https://api.bilibili.com/x/web-interface/wbi/search/type?{}",
        query
    );
    grain_pdk::log(
        grain_pdk::LogLevel::Debug,
        &format!("[bili-wbi] signed url: {}", url),
    );
    let resp = grain_pdk::http("GET", &url, &headers_json(), "").await?;
    let v = resp.json()?;
    let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    if code != 0 {
        let msg = v
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown");
        return Err(Error::msg(format!("bili search error {}: {}", code, msg)));
    }
    let empty = Vec::new();
    let list = v
        .pointer("/data/result")
        .and_then(|x| x.as_array())
        .unwrap_or(&empty);
    let items: Vec<Value> = list
        .iter()
        .filter_map(|e| e.get("bvid")?.as_str().map(|b| map_video(b, e)))
        .collect();
    Ok(Value::Array(items))
}

async fn do_latest(page: u32) -> Result<Value> {
    // 热门榜，不需要 wbi 签名，但同样走沙箱化 HTTP
    let url = format!(
        "https://api.bilibili.com/x/web-interface/popular?ps=20&pn={}",
        page.max(1)
    );
    let resp = grain_pdk::http("GET", &url, &headers_json(), "").await?;
    let v = resp.json()?;
    let empty = Vec::new();
    let list = v
        .pointer("/data/list")
        .and_then(|x| x.as_array())
        .unwrap_or(&empty);
    let items: Vec<Value> = list
        .iter()
        .filter_map(|e| {
            let bvid = e.get("bvid")?.as_str()?;
            let mut item = map_video(bvid, e);
            if let Some(owner) = e.pointer("/owner/name").and_then(|x| x.as_str()) {
                item["update"] = json!(owner);
            }
            Some(item)
        })
        .collect();
    Ok(Value::Array(items))
}

#[plugin_fn]
pub async fn latest(page: u32) -> Result<Value> {
    do_latest(page).await
}

#[plugin_fn]
pub async fn search(keyword: String) -> Result<Value> {
    do_search(&keyword).await
}

#[cfg(test)]
mod tests {
    use super::*;

    // 参考向量由 Python（hashlib + urllib）独立计算，勿用本文件函数反推
    const IMG_KEY: &str = "7cd084941338484aae1ad9425b84077c";
    const SUB_KEY: &str = "4932cddb0c149eddcf9021ffd11a5d96";

    #[test]
    fn mixin_key_matches_reference() {
        assert_eq!(
            derive_mixin_key(
                "e412c765cb7d0f1bd7a2f0a7f5a3f27a",
                "2c1e2f7b9a3d4c5e8f6a0b1c2d3e4f5a"
            ),
            "5ea1bc72b67a32ce3d7fcb322f1b07af"
        );
    }

    #[test]
    fn mixin_key_reference_vector() {
        assert_eq!(
            derive_mixin_key(IMG_KEY, SUB_KEY),
            "dd1d1124a93c10e2444f931a704b4dc8"
        );
    }

    #[test]
    fn key_from_url_parsing() {
        assert_eq!(
            key_from_url("https://i0.hdslb.com/bfs/wbi/7cd084941338484aae1ad9425b84077c.png"),
            Some("7cd084941338484aae1ad9425b84077c".to_string())
        );
        assert_eq!(
            key_from_url("https://i0.hdslb.com/bfs/wbi/nothex.png"),
            None
        );
    }

    #[test]
    fn wbi_sign_matches_reference() {
        let params = vec![
            ("search_type".to_string(), "video".to_string()),
            ("keyword".to_string(), "测试".to_string()),
            ("page".to_string(), "1".to_string()),
        ];
        let signed = wbi_sign(params, "dd1d1124a93c10e2444f931a704b4dc8", 1700000000);
        assert_eq!(
            signed,
            "keyword=%E6%B5%8B%E8%AF%95&page=1&search_type=video&wts=1700000000&w_rid=963062c4641f911605da960a007800b4"
        );
    }

    #[test]
    fn strip_tags_removes_em() {
        assert_eq!(
            strip_tags(r#"<em class="keyword">鬼灭</em>之刃"#),
            "鬼灭之刃"
        );
    }
}
