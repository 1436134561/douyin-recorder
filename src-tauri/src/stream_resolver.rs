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

/// 解析结果（含流地址 + 房间元数据）
#[derive(Debug, Clone)]
pub struct ResolvedStream {
    /// 最佳 FLV 流地址（优先原画 OR4，其次 HD）
    pub flv: String,
    /// HLS (m3u8) 地址（可选）
    pub hls: Option<String>,
    /// 房间元数据（主播昵称、房间标题等）
    pub meta: Option<RoomMeta>,
}

/// 从直播间提取的元数据
#[derive(Debug, Clone, serde::Serialize)]
pub struct RoomMeta {
    /// 主播昵称
    pub nickname: String,
    /// 房间标题
    pub title: String,
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
pub fn extract_web_rid(input: &str) -> Result<String> {
    let text = input.trim();

    // 纯数字房间号
    if text.chars().all(|c| c.is_ascii_digit()) && !text.is_empty() {
        return Ok(text.to_string());
    }

    // 从 URL 中提取
    let patterns = [
        r"live\.douyin\.com/([^/?#]+)",
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

/// HTTP 客户端（带 cookie jar + 浏览器 UA + 完整请求头）
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
/// 返回 ttwid 字符串用于后续请求
async fn obtain_ttwid(client: &reqwest::Client) -> Result<String> {
    // 方案 1：POST ttwid 注册接口获取 cookie
    let payload = serde_json::json!({
        "region": "cn",
        "aid": 1768,
        "needFid": false,
        "service": "www.ixigua.com",
        "migrate_info": {"ticket": "", "source": "node"},
        "cbUrlProtocol": "https",
        "union": true,
    });

    let resp = client
        .post("https://ttwid.bytedance.com/ttwid/union/register/")
        .header("Content-Type", "application/json")
        .header("Origin", "https://www.douyin.com")
        .header("Referer", "https://www.douyin.com/")
        .body(payload.to_string())
        .send()
        .await
        .map_err(|e| anyhow!("请求 ttwid 注册接口失败: {}", e))?;

    // 从 Set-Cookie 或响应体中提取 ttwid
    if let Some(cookie_hdr) = resp.headers().get("set-cookie") {
        let cookie_str = cookie_hdr.to_str().unwrap_or("");
        if let Some(ttwid) = extract_cookie_value(cookie_str, "ttwid") {
            return Ok(ttwid);
        }
    }

    // 尝试从响应体 JSON 中提取
    let body = resp.text().await.unwrap_or_default();
    if let Ok(json) = serde_json::from_str::<Value>(&body) {
        if let Some(token) = json.get("data").and_then(|d| d.get("token")).and_then(|t| t.as_str()) {
            // 构造 ttwid cookie 值格式
            return Ok(format!("1{}", token));
        }
    }

    // 方案 2：访问 live.douyin.com 触发 cookie
    let resp2 = client
        .get("https://live.douyin.com/")
        .header("Accept", "text/html,application/xhtml+xml")
        .send()
        .await;

    if let Ok(r) = resp2 {
        if let Some(cookie_hdr) = r.headers().get("set-cookie") {
            let cookie_str = cookie_hdr.to_str().unwrap_or("");
            if let Some(ttwid) = extract_cookie_value(cookie_str, "ttwid") {
                return Ok(ttwid);
            }
        }
    }

    // 返回空字符串表示未获取到（后续请求可能仍能工作）
    Ok(String::new())
}

/// 从 Set-Cookie header 字符串中提取指定 cookie 的值
fn extract_cookie_value(cookie_header: &str, name: &str) -> Option<String> {
    for part in cookie_header.split(';') {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix(name) {
            if let Some(val) = rest.strip_prefix('=') {
                return Some(val.trim().to_string());
            }
        }
    }
    None
}

/// 从 Webcast API 响应中提取房间元数据（多重路径宽松解析，任一命中即返回）
fn extract_meta_from_api(data: &Value) -> Option<RoomMeta> {
    let data_obj = data.get("data")?;

    // 尝试多个路径获取主播昵称
    let nickname = data_obj
        .get("user")
        .and_then(|u| u.get("nickname"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            data_obj
                .get("anchor")
                .and_then(|a| a.get("nickname"))
                .and_then(|v| v.as_str())
        })
        .or_else(|| {
            data_obj
                .get("room")
                .and_then(|r| r.get("owner"))
                .and_then(|o| o.get("nickname"))
                .and_then(|v| v.as_str())
        })
        .or_else(|| {
            data_obj
                .get("room")
                .and_then(|r| r.get("anchor"))
                .and_then(|a| a.get("nickname"))
                .and_then(|v| v.as_str())
        })
        .map(|s| s.to_string());

    // 尝试多个路径获取房间标题
    let title = data_obj
        .get("room")
        .and_then(|r| r.get("title"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            data_obj
                .get("room")
                .and_then(|r| r.get("dynamic_content"))
                .and_then(|d| d.get("title"))
                .and_then(|v| v.as_str())
        })
        .map(|s| s.to_string());

    // 任一字段获取到即返回
    match (nickname, title) {
        (Some(n), t) if !n.is_empty() => Some(RoomMeta {
            nickname: n,
            title: t.unwrap_or_default(),
        }),
        _ => None,
    }
}

/// 调用 Webcast API 获取流地址（首选方案）
async fn fetch_webcast_api(
    web_rid: &str,
    client: &reqwest::Client,
) -> Result<(HashMap<String, String>, Option<RoomMeta>)> {
    // 获取 ttwid cookie
    let ttwid = obtain_ttwid(client).await.ok();

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

    let mut req_builder = client
        .get(&url)
        .header("Referer", "https://live.douyin.com/")
        .header("Accept", "application/json, text/plain, */*");

    // 附上 ttwid cookie（如果有）
    if let Some(ref tid) = ttwid {
        if !tid.is_empty() {
            req_builder = req_builder.header("Cookie", format!("ttwid={}", tid));
        }
    }

    let resp = req_builder
        .send()
        .await
        .map_err(|e| anyhow!("请求 Webcast API 失败: {}", e))?;

    let status = resp.status();
    let body = resp.text().await.map_err(|e| anyhow!("读取响应失败: {}", e))?;

    // 先尝试解析为 JSON
    let data: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => {
            // 非 JSON 响应（可能是 HTML 重定向或错误页）
            return Err(anyhow!(
                "API 返回非 JSON 数据（HTTP {}），可能需要更新解析逻辑。响应前 200 字节: {}",
                status,
                &body[..body.len().min(200)]
            ));
        }
    };

    // 检查 API 返回状态
    let status_code = data.get("status_code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if status_code != 0 {
        let msg = data
            .get("status_msg")
            .and_then(|v| v.as_str())
            .unwrap_or("未知错误");
        return Err(anyhow!("API 返回错误({}): {} (主播可能未在直播)", status_code, msg));
    }

    // 提取元数据
    let meta = extract_meta_from_api(&data);

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
        Ok((result, meta))
    }
}

/// 方案 2：从页面 HTML 的 RENDER_DATA 提取流地址和元数据
async fn fetch_page_render_data(
    web_rid: &str,
    client: &reqwest::Client,
) -> Result<(HashMap<String, String>, Option<RoomMeta>)> {
    // 构造直播间页面 URL
    let page_url = format!("https://live.douyin.com/{}", web_rid);

    let resp = client
        .get(&page_url)
        .header("Referer", "https://www.douyin.com/")
        .header("Accept", "text/html,application/xhtml+xml")
        .send()
        .await
        .map_err(|e| anyhow!("请求直播间页面失败: {}", e))?;

    let html = resp.text().await.map_err(|e| anyhow!("读取页面失败: {}", e))?;

    // 提取 RENDER_DATA JSON
    let render_data_str = extract_render_data(&html)?;

    let data: Value = serde_json::from_str(&render_data_str)
        .map_err(|e| anyhow!("解析 RENDER_DATA JSON 失败: {}", e))?;

    // 提取元数据
    let meta = extract_meta_from_render_data(&data);

    // 查找流地址
    let mut flv_urls = HashMap::new();
    let mut hls_urls = HashMap::new();
    recursive_find_streams(&data, &mut flv_urls, &mut hls_urls);

    let mut result = HashMap::new();
    for (k, v) in flv_urls {
        result.insert(format!("flv:{}", k), v);
    }
    for (k, v) in hls_urls {
        result.insert(format!("hls:{}", k), v);
    }

    if result.is_empty() {
        Err(anyhow!("页面 RENDER_DATA 中未找到流地址"))
    } else {
        Ok((result, meta))
    }
}

/// 从 HTML 中提取 RENDER_DATA 内容
fn extract_render_data(html: &str) -> Result<String> {
    // 匹配 <script id="RENDER_DATA" type="application/json">...</script>
    let re = Regex::new(r#"<script\s+id="RENDER_DATA"\s+type="application/json">([^<]+)</script>"#)
        .map_err(|e| anyhow!("编译 RENDER_DATA 正则失败: {}", e))?;

    if let Some(cap) = re.captures(html) {
        let raw = cap
            .get(1)
            .ok_or_else(|| anyhow!("RENDER_DATA 内容为空"))?
            .as_str();

        // RENDER_DATA 是 URL 编码的 JSON
        match urlencoding::decode(raw) {
            Ok(decoded) => Ok(decoded.into_owned()),
            Err(_) => Ok(raw.to_string()),
        }
    } else {
        Err(anyhow!("页面中未找到 RENDER_DATA（可能被反爬拦截）"))
    }
}

/// 从 RENDER_DATA JSON 中提取房间元数据
fn extract_meta_from_render_data(data: &Value) -> Option<RoomMeta> {
    // RENDER_DATA 结构: { "app": { "initialState": { "roomStore": { "roomInfo": { ... } }, "userStore": ... } } }
    let initial_state = data
        .get("app")?
        .get("initialState")?;

    // 尝试从 roomStore 获取
    let room_info = initial_state
        .get("roomStore")?
        .get("roomInfo")?
        .get("room")?;

    let nickname = room_info
        .get("anchor")?
        .get("nickName")?
        .as_str()?
        .to_string();
    let title = room_info.get("title")?.as_str()?.to_string();

    Some(RoomMeta { nickname, title })
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

            // 递归遍历其他字段
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

/// 公开接口：解析用户输入为真实流地址 + 元数据
///
/// # Arguments
/// * `input` - 抖音直播间 URL、短链接或纯房间号
///
/// # Returns
/// `ResolvedStream` 包含最佳 FLV、可选 HLS 地址和房间元数据
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
            meta: None,
        });
    }

    let web_rid = extract_web_rid(input)?;
    let client = http_client();

    // 收集每个策略的失败原因，最后一并告诉用户（解决「看不到真错误」的问题）
    let mut attempts: Vec<(String, String)> = Vec::new(); // (策略名, 错误)

    // 方案 1: Webcast API
    match fetch_webcast_api(&web_rid, &client).await {
        Ok((urls, meta)) => {
            if let Some(flv) = best_flv(&urls) {
                return Ok(ResolvedStream {
                    flv,
                    hls: best_hls(&urls),
                    meta,
                });
            }
            attempts.push(("Webcast API".into(), "未返回流地址".into()));
        }
        Err(e) => {
            attempts.push(("Webcast API".into(), format!("{}", e)));
        }
    }

    // 方案 2: 页面 HTML RENDER_DATA（备选）
    match fetch_page_render_data(&web_rid, &client).await {
        Ok((urls, meta)) => {
            if let Some(flv) = best_flv(&urls) {
                return Ok(ResolvedStream {
                    flv,
                    hls: best_hls(&urls),
                    meta,
                });
            }
            attempts.push(("页面 RENDER_DATA".into(), "未返回流地址".into()));
        }
        Err(e) => {
            attempts.push(("页面 RENDER_DATA".into(), format!("{}", e)));
        }
    }

    // 两个策略都失败 → 详细列出每个策略的错误，方便用户/排查定位
    let detail = attempts
        .iter()
        .map(|(name, err)| format!("  · {}：{}", name, err))
        .collect::<Vec<_>>()
        .join("\n");

    Err(anyhow!(
        "无法获取直播流地址（房间 {}）。请确认：\n\
         1. 主播正在直播（抖音部分直播间会做地域限制或风控）\n\
         2. 房间号/链接正确\n\
         3. 网络连接正常\n\
         \n\
         各解析策略失败原因：\n{}",
        web_rid,
        detail
    ))
}

// ─── 直播状态探测（用于 monitor 轮询） ───

/// 直播状态探测器：复用 reqwest::Client + ttwid cookie，避免每 30s 重新注册触发风控
///
/// 用法：每个房间共享一个实例即可（内部 client/ttwid 在多次 is_live 调用间复用）。
/// 注意：当前实现是 `&mut self`（非 Send 友好），调用方需要独占访问。Monitor 线程独占持有，故 OK。
pub struct LiveProbe {
    client: reqwest::Client,
    ttwid: Option<String>,
    initialized: bool,
}

impl Default for LiveProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveProbe {
    pub fn new() -> Self {
        Self {
            client: http_client(),
            ttwid: None,
            initialized: false,
        }
    }

    /// 首次调用时注册 ttwid，后续调用复用缓存
    async fn ensure_init(&mut self) -> Result<()> {
        if !self.initialized {
            // obtain_ttwid 即使失败也不阻断（返回空字符串时仍可继续探测）
            self.ttwid = obtain_ttwid(&self.client).await.ok();
            self.initialized = true;
        }
        Ok(())
    }

    /// 探测指定房间是否正在直播
    /// - `Ok(true)`：开播中（API 返回流地址）
    /// - `Ok(false)`：未开播 / 已下播（status_code != 0 或无流地址）
    /// - `Err(_)`：网络错误 / JSON 解析失败（区分于「未开播」）
    pub async fn is_live(&mut self, web_rid: &str) -> Result<bool> {
        self.ensure_init().await?;
        probe_live_via_api(web_rid, &self.client, self.ttwid.as_deref()).await
    }
}

/// 仅探测直播状态：调用 Webcast API，只判断是否在播（不返回流地址）
///
/// 抽取自 fetch_webcast_api 以减少网络/解析开销（不递归提取流地址，只看 status_code 和有无 url 字段）
async fn probe_live_via_api(
    web_rid: &str,
    client: &reqwest::Client,
    ttwid: Option<&str>,
) -> Result<bool> {
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

    let mut req_builder = client
        .get(&url)
        .header("Referer", "https://live.douyin.com/")
        .header("Accept", "application/json, text/plain, */*");

    if let Some(tid) = ttwid {
        if !tid.is_empty() {
            req_builder = req_builder.header("Cookie", format!("ttwid={}", tid));
        }
    }

    let resp = req_builder
        .send()
        .await
        .map_err(|e| anyhow!("探测直播状态请求失败: {}", e))?;

    let status = resp.status();
    let body = resp.text().await.map_err(|e| anyhow!("读取探测响应失败: {}", e))?;

    let data: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => {
            return Err(anyhow!(
                "Webcast API 返回非 JSON（HTTP {}），可能触发风控。响应前 200 字节: {}",
                status,
                &body[..body.len().min(200)]
            ));
        }
    };

    let status_code = data.get("status_code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if status_code != 0 {
        // status_code != 0 明确表示未开播（或已下播）
        return Ok(false);
    }

    // 检查 data.* 字段里是否有流地址（任一 flv / hls 即可判定开播）
    let mut flv_urls = HashMap::new();
    let mut hls_urls = HashMap::new();
    if let Some(data_val) = data.get("data") {
        recursive_find_streams(data_val, &mut flv_urls, &mut hls_urls);
    }

    Ok(!flv_urls.is_empty() || !hls_urls.is_empty())
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

    #[test]
    fn test_extract_cookie_value() {
        let header = "ttwid=1%7Cabc123_xyz; Path=/; Domain=.bytedance.com; Max-Age=31536000";
        assert_eq!(
            extract_cookie_value(header, "ttwid"),
            Some("1%7Cabc123_xyz".to_string())
        );
        assert_eq!(extract_cookie_value(header, "other"), None);
    }

    #[test]
    fn test_extract_render_data() {
        let html = r#"<!DOCTYPE html><html><body>
<script id="RENDER_DATA" type="application/json">%7B%22app%22%3A%7B%7D%7D</script>
</body></html>"#;
        let result = extract_render_data(html);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "{\"app\":{}}");
    }
}
