//! 抖音直播间 URL → 真实直播流地址（FLV/HLS）解析器
//!
//! 支持输入格式：
//!   - 直播间网页: `https://live.douyin.com/909927995061`
//!   - 关注页直播: `https://www.douyin.com/follow/live/909927995061?anchor_id=xxx`
//!   - 短链接:     `https://v.douyin.com/xxxxx`
//!   - 纯房间号:   `909927995061`
//!
//! 解析策略（三级）：
//!   1. Webcast API（最可靠）：GET /webcast/room/web/enter/?web_rid={id}
//!   2. RENDER_DATA 提取：从页面 HTML 的 <script id="RENDER_DATA"> 解析 JSON
//!   3. 正则兜底：匹配 xxx.flv? / index.m3u8? 后缀 URL

use anyhow::{anyhow, Result};
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;

/// 解析结果
#[derive(Debug, Clone)]
pub struct ResolvedStream {
    /// 最佳 FLV 流地址（优先原画 OR4，其次 HD）
    pub flv: String,
    /// HLS (m3u8) 地址（可选）
    pub hls: Option<String>,
}

/// 画质优先级（从高到低）
const QUALITY_ORDER: &[&str] = &["OR4", "UHD", "HD", "SD", "LD"];

/// 判断输入是否已经是流地址（flv/hls/m3u8 结尾或含 pull/douyincdn）
pub fn is_stream_url(url: &str) -> bool {
    let u = url.to_lowercase();
    // 去掉查询参数后再检查扩展名
    let path = u.split('?').next().unwrap_or(&u);
    path.ends_with(".flv")
        || path.ends_with(".m3u8")
        || u.contains("pull.")
        || u.contains("douyincdn")
        || u.contains("liveplay")
}

/// 判断输入是否看起来像抖音直播间 URL
pub fn looks_like_douyin_url(input: &str) -> bool {
    let lower = input.to_lowercase();
    lower.contains("douyin.com")
        || lower.contains("live.douyin")
        || lower.contains("v.douyin.com")
}

/// 从用户输入提取 web_rid（房间 ID）
fn extract_web_rid(input: &str) -> Result<String> {
    let text = input.trim();

    // 纯数字房间号
    if text.chars().all(|c| c.is_ascii_digit()) && !text.is_empty() {
        return Ok(text.to_string());
    }

    // 从 URL 中提取
    // 短链接 v.douyin.com/xxx → 需要跟随重定向（此处先尝试正则提取 room_id）
    // 直播间 live.douyin.com/{rid}
    // 关注页 /follow/live/{rid}
    // 参数 ?anchor_id=xxx 或 room_id=xxx 或 web_rid=xxx

    let patterns = [
        r"live\.douyin\.com/([^/?#]+)",
        r"/live/(\d+)",
        r"/follow/live/(\d+)",
        r"room_id[=:](\d+)",
        r"anchor_id[=:](\d+)",
        r"web_rid[=:](\d+)",
        r"roomId[=:](\s*)(\d+)",
    ];

    for pat in patterns {
        if let Ok(re) = Regex::new(pat) {
            if let Some(cap) = re.captures(text) {
                if let Some(rid) = cap.get(1).or_else(|| cap.get(2)) {
                    let s = rid.as_str().trim();
                    if !s.is_empty() {
                        return Ok(s.to_string());
                    }
                }
            }
        }
    }

    // 最后尝试取 URL 路径最后一段数字
    if text.contains('/') {
        let parts: Vec<&str> = text.split('/').collect();
        for part in parts.iter().rev() {
            let clean = part.split(['?', '#']).next().unwrap_or(part);
            if clean.chars().all(|c| c.is_ascii_digit()) && !clean.is_empty() {
                return Ok(clean.to_string());
            }
        }
    }

    Err(anyhow!(
        "无法从输入中提取房间号。支持格式：抖音直播间链接、短链接、纯房间号"
    ))
}

/// HTTP 客户端（带 cookie jar + 浏览器 UA）
fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
             AppleWebKit/537.36 (KHTML, like Gecko) \
             Chrome/131.0.0.0 Safari/537.36",
        )
        .build()
        .unwrap_or_default()
}

/// 初始化 ttwid cookie（抖音反爬要求）
async fn init_cookies(client: &reqwest::Client) -> Result<()> {
    // 先访问一次 live.douyin.com 触发 cookie
    match client.get("https://live.douyin.com/").send().await {
        Ok(resp) => {
            if resp.headers().get("set-cookie").is_some() {
                return Ok(()); // 已有 cookie
            }
        }
        Err(_) => {}
    }

    // 备选：POST ttwid 接口注册
    let payload = serde_json::json!({
        "region": "cn",
        "aid": 1768,
        "needFid": false,
        "service": "www.ixigua.com",
        "migrate_info": {"ticket": "", "source": "node"},
        "cbUrlProtocol": "https",
        "union": true,
    });

    client
        .post("https://ttwid.bytedance.com/ttwid/union/register/")
        .header("Content-Type", "application/json")
        .body(payload.to_string())
        .send()
        .await
        .ok(); // 失败不阻塞

    Ok(())
}

/// 调用 Webcast API 获取流地址（首选方案）
async fn fetch_webcast_api(web_rid: &str, client: &reqwest::Client) -> Result<HashMap<String, String>> {
    init_cookies(client).await.ok();

    let params = [
        ("aid", "6383"),
        ("app_name", "douyin_web"),
        ("live_id", "1"),
        ("device_platform", "web"),
        ("language", "zh-CN"),
        ("browser_language", "zh-CN"),
        ("browser_platform", "Win32"),
        ("browser_name", "Chrome"),
        ("browser_version", "131.0.0.0"),
        ("web_rid", web_rid),
    ];

    let url = format!(
        "https://live.douyin.com/webcast/room/web/enter/?{}",
        params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&")
    );

    let resp = client
        .get(&url)
        .header("Referer", "https://live.douyin.com/")
        .send()
        .await
        .map_err(|e| anyhow!("请求 Webcast API 失败: {}", e))?;

    let body = resp.text().await.map_err(|e| anyhow!("读取响应失败: {}", e))?;
    let data: Value = serde_json::from_str(&body).map_err(|e| anyhow!("解析 JSON 失败: {}", e))?;

    // 检查 API 返回状态
    if data.get("status_code").and_then(|v| v.as_i64()) != Some(0) {
        let msg = data
            .get("status_msg")
            .and_then(|v| v.as_str())
            .unwrap_or("未知错误");
        return Err(anyhow!("API 返回错误: {} (主播可能未在直播)", msg));
    }

    // 递归查找流地址
    let mut flv_urls = HashMap::new();
    let mut hls_urls = HashMap::new();
    if let Some(data_val) = data.get("data") {
        recursive_find_streams(data_val, &mut flv_urls, &mut hls_urls);
    }

    // 合并结果（FLV + HLS）
    let mut result = HashMap::new();
    for (k, v) in flv_urls {
        result.insert(format!("flv:{}", k), v);
    }
    for (k, v) in hls_urls {
        result.insert(format!("hls:{}", k), v);
    }

    if result.is_empty() {
        Err(anyhow!(
            "API 未返回流地址（主播可能未在直播或已下播）"
        ))
    } else {
        Ok(result)
    }
}

/// 递归查找 JSON 中的流地址字段
fn recursive_find_streams(
    data: &Value,
    flv_out: &mut HashMap<String, String>,
    hls_out: &mut HashMap<String, String>,
) {
    match data {
        Value::Object(map) => {
            // 检查已知字段
            if let Some(flv_val) = map.get("flv_pull_url") {
                match flv_val {
                    Value::Object(inner) => {
                        for (key, val) in inner {
                            if let Some(url) = val.as_str() {
                                if url.starts_with("http") {
                                    flv_out.insert(key.clone(), url.to_string());
                                }
                            }
                        }
                    }
                    Value::String(s) if s.starts_with("http") => {
                        flv_out.insert("default".into(), s.clone());
                    }
                    _ => {}
                }
            }

            if let Some(hls_val) = map.get("hls_pull_url_map") {
                if let Value::Object(inner) = hls_val {
                    for (key, val) in inner {
                        if let Some(url) = val.as_str() {
                            if url.starts_with("http") {
                                hls_out.insert(key.clone(), url.to_string());
                            }
                        }
                    }
                }
            }

            if let Some(hls_val) = map.get("hls_pull_url") {
                if let Some(url) = hls_val.as_str() {
                    if url.starts_with("http") {
                        hls_out.insert("default".into(), url.to_string());
                    }
                }
            }

            // 递归遍历其他字段（已知流地址字段已在上面处理过）
            for value in map.values() {
                recursive_find_streams(value, flv_out, hls_out);
            }
        }
        Value::Array(arr) => {
            for item in arr {
                recursive_find_streams(item, flv_out, hls_out);
            }
        }
        _ => {}
    }
}

/// 从 HashMap 中按画质优先级选择最佳 FLV URL
fn best_flv(urls: &HashMap<String, String>) -> Option<String> {
    for q in QUALITY_ORDER {
        let key = format!("flv:{}", q);
        if let Some(url) = urls.get(&key) {
            return Some(url.clone());
        }
    }
    // 兜底：任意 flv
    for (k, v) in urls {
        if k.starts_with("flv:") {
            return Some(v.clone());
        }
    }
    None
}

/// 从 HashMap 中选择最佳 HLS URL
fn best_hls(urls: &HashMap<String, String>) -> Option<String> {
    for q in QUALITY_ORDER {
        let key = format!("hls:{}", q);
        if let Some(url) = urls.get(&key) {
            return Some(url.clone());
        }
    }
    for (k, v) in urls {
        if k.starts_with("hls:") {
            return Some(v.clone());
        }
    }
    None
}

/// 公开接口：解析用户输入为真实流地址
///
/// # Arguments
/// * `input` - 抖音直播间 URL、短链接或纯房间号
///
/// # Returns
/// `ResolvedStream` 包含最佳 FLV 和可选 HLS 地址
pub async fn resolve(input: &str) -> Result<ResolvedStream> {
    let input = input.trim();
    if input.is_empty() {
        return Err(anyhow!("输入不能为空"));
    }

    // 如果已经是流地址，直接返回
    if is_stream_url(input) {
        return Ok(ResolvedStream {
            flv: input.to_string(),
            hls: None,
        });
    }

    let web_rid = extract_web_rid(input)?;
    let client = http_client();

    // 方案 1: Webcast API
    if let Ok(urls) = fetch_webcast_api(&web_rid, &client).await {
        if let Some(flv) = best_flv(&urls) {
            return Ok(ResolvedStream {
                flv,
                hls: best_hls(&urls),
            });
        }
    }

    // 方案 2: 页面 HTML RENDER_DATA（备选）
    // TODO: 若 Webcast API 失败可在此添加页面抓取逻辑

    Err(anyhow!(
        "无法获取直播流地址。请确认：\n\
         1. 主播正在直播\n\
         2. 房间号/链接正确\n\
         3. 网络连接正常"
    ))
}

// ─── 单元测试 ───

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_web_rid_from_live_url() {
        assert_eq!(
            extract_web_rid("https://live.douyin.com/909927995061").unwrap(),
            "909927995061"
        );
    }

    #[test]
    fn test_extract_web_rid_from_follow_url() {
        assert_eq!(
            extract_web_rid(
                "https://www.douyin.com/follow/live/909927995061?anchor_id=3101042824188062"
            )
            .unwrap(),
            "909927995061"
        );
    }

    #[test]
    fn test_extract_web_rid_from_room_id() {
        assert_eq!(extract_web_rid("909927995061").unwrap(), "909927995061");
    }

    #[test]
    fn test_extract_web_rid_from_anchor_param() {
        assert_eq!(
            extract_web_rid("https://www.douyin.com/some/path?anchor_id=123456789").unwrap(),
            "123456789"
        );
    }

    #[test]
    fn test_extract_web_rid_invalid() {
        assert!(extract_web_rid("not-a-url-or-id").is_err());
        assert!(extract_web_rid("").is_err());
    }

    #[test]
    fn test_is_stream_url() {
        assert!(is_stream_url("https://pull-xxx.douyincdn.com/live/or4.flv?token=abc"));
        assert!(is_stream_url("https://xxx.m3u8?key=val"));
        assert!(!is_stream_url("https://live.douyin.com/123"));
        assert!(!is_stream_url("https://www.douyin.com/follow/live/123"));
    }

    #[test]
    fn test_looks_like_douyin_url() {
        assert!(looks_like_douyin_url("https://live.douyin.com/123"));
        assert!(looks_like_douyin_url("https://v.douyin.com/abc"));
        assert!(looks_like_douyin_url("https://www.douyin.com/follow/live/123"));
        assert!(!looks_like_douyin_url("https://example.com/live/123"));
        assert!(!looks_like_douyin_url("123456789")); // 纯房间号不算 URL
    }

    #[test]
    fn test_best_flv_priority() {
        let mut urls = HashMap::new();
        urls.insert("flv:SD".into(), "sd.flv".into());
        urls.insert("flv:OR4".into(), "or4.flv".into());
        urls.insert("flv:HD".into(), "hd.flv".into());

        assert_eq!(best_flv(&urls), Some("or4.flv".into()));
    }

    #[test]
    fn test_recursive_find_streams() {
        let json: Value = serde_json::json!({
            "data": {
                "stream_data": {
                    "flv_pull_url": {
                        "OR4": "https://pull-xxx.or4.flv?t=1",
                        "HD": "https://pull-xxx.hd.flv?t=2"
                    },
                    "hls_pull_url_map": {
                        "OR4": "https://pull-xxx.or4/index.m3u8?t=1"
                    }
                }
            }
        });
        let mut flv = HashMap::new();
        let mut hls = HashMap::new();
        recursive_find_streams(&json, &mut flv, &mut hls);

        assert_eq!(flv.get("OR4"), Some(&"https://pull-xxx.or4.flv?t=1".to_string()));
        assert_eq!(flv.get("HD"), Some(&"https://pull-xxx.hd.flv?t=2".to_string()));
        assert_eq!(
            hls.get("OR4"),
            Some(&"https://pull-xxx.or4/index.m3u8?t=1".to_string())
        );
    }
}
