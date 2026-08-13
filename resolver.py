"""
抖音直播间解析。

解析路径（按优先级）：
1. 直播页 HTML RENDER_DATA 直采（通常无需 a_bogus）。
2. webcast/room/web/enter 接口（真实 a_bogus 签名，见 ab_sign.py；RENDER_DATA 无流时自动回退）。
3. 全失败 -> raise RuntimeError（界面显示「解析失败」+ 原因）。

对外暴露：
    resolve(url, cookie="", proxy="") -> dict | None
    { "room_id", "web_rid", "name", "streams": [ {url, type} ], "source" }
"""
from __future__ import annotations

import json
import re
import urllib.parse
from typing import Optional

try:
    import requests
except ImportError:  # pragma: no cover
    requests = None

DEFAULT_UA = (
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36"
)

FLV_RE = re.compile(r"https?://[^\s\"'\\]+?\.(?:flv|flv\?)[^\s\"'\\]*", re.I)
HLS_RE = re.compile(r"https?://[^\s\"'\\]+?\.m3u8[^\s\"'\\]*", re.I)
# 抖音 pull 域名
PULL_HOST_RE = re.compile(r"https?://[^\s\"'\\]*?(?:pull-flv|pull-hls|pull-flv-l|pull\d|live\d|al\d|ty\d|hw\d)[^\s\"'\\]*", re.I)

# 昵称 / 房间号提取
NICK_RE = re.compile(r'"nickname"\s*:\s*"([^"\\]+)"')
ROOMID_RE = re.compile(r'"(?:room_id|owner_room_id)"\s*:\s*"?(\d+)"?')

# RENDER_DATA 常见变体（抖音页面结构随版本变化）
RENDER_DATA_RES = [
    re.compile(r'<script\s+id="RENDER_DATA"\s+type="application/json">(.*?)</script>', re.S | re.I),
    re.compile(r'<script\s+id="RENDER_DATA">(.*?)</script>', re.S | re.I),
    re.compile(r'RENDER_DATA\s*=\s*"(.*?)"\s*</script>', re.S | re.I),
]

# webcast/web/enter 接口地址
ENTER_API = "https://live.douyin.com/webcast/room/web/enter/"


def _require_requests():
    if requests is None:
        raise RuntimeError("缺少依赖 requests，请先 `pip install requests`")


def normalize_room_input(url: str) -> str:
    """把用户粘贴的各种输入规整为直播页 URL。"""
    s = (url or "").strip()
    if not s:
        return s
    # 纯数字房间号
    if re.fullmatch(r"\d{6,20}", s):
        return f"https://live.douyin.com/{s}"
    # 短链 / 分享链
    m = re.search(r"live\.douyin\.com/([0-9A-Za-z_]+)", s)
    if m:
        return f"https://live.douyin.com/{m.group(1)}"
    if not s.startswith("http"):
        s = "https://" + s
    return s


def _decode_render_data(html: str) -> Optional[dict]:
    for pat in RENDER_DATA_RES:
        m = pat.search(html)
        if not m:
            continue
        raw = m.group(1).strip()
        try:
            decoded = urllib.parse.unquote(raw)
            return json.loads(decoded)
        except Exception:
            continue
    # 兜底：转义 JSON（\\"state\\":... 结构）
    try:
        m = re.search(r'(\{\\"state\\":.*?)]\\n"]\))', html, re.S)
        if m:
            cleaned = m.group(1).replace("\\", "").replace(r"u0026", "&")
            return json.loads(cleaned)
    except Exception:
        pass
    return None


def _walk_strings(obj, out: list):
    """递归收集 JSON 中的字符串值。"""
    if isinstance(obj, str):
        out.append(obj)
    elif isinstance(obj, dict):
        for v in obj.values():
            _walk_strings(v, out)
    elif isinstance(obj, list):
        for v in obj:
            _walk_strings(v, out)


def _find_pull_data(obj):
    """递归查找含 pull_data 的字典（live_core_sdk_data 位置随版本变化）。"""
    if isinstance(obj, dict):
        if "pull_data" in obj:
            return obj["pull_data"]
        for v in obj.values():
            r = _find_pull_data(v)
            if r is not None:
                return r
    elif isinstance(obj, list):
        for v in obj:
            r = _find_pull_data(v)
            if r is not None:
                return r
    return None


def _parse_stream_data(raw) -> list:
    """解析 stream_data（str 或 dict）为推流地址列表。

    stream_data 反序列化后常见结构：
        {"data": {"origin": {"main": {...}}, "fullhd1": {"main": {...}}, ...}}
        {"data": {"data": {...}, "origin": {"main": {...}}}}
    统一遍历所有画质节点，取 hls/flv 列表。
    """
    if isinstance(raw, str):
        try:
            raw = json.loads(raw)
        except Exception:
            return []
    if not isinstance(raw, dict):
        return []
    streams: list = []
    seen = set()

    def add(u: str, typ: str):
        u = (u or "").strip()
        if not u or u in seen:
            return
        seen.add(u)
        streams.append({"url": u, "type": typ})

    data_node = raw.get("data") if isinstance(raw.get("data"), dict) else raw
    # 遍历 data_node 下所有画质键（origin / fullhd1 / sd1 ... / data）
    for q in data_node.values():
        if not isinstance(q, dict):
            continue
        main = q.get("main") if isinstance(q.get("main"), dict) else q
        if not isinstance(main, dict):
            continue
        for f in main.get("flv") or []:
            add(f, "flv")
        for h in main.get("hls") or []:
            add(h, "hls")
    return streams


def _extract_streams_structured(render: dict):
    """从 live_core_sdk_data.pull_data.stream_data 结构化提取推流地址（最可靠的路径）。"""
    pd = _find_pull_data(render)
    if not pd:
        return []
    return _parse_stream_data(pd.get("stream_data"))


def _extract_streams(render: dict):
    # 1) 结构化：live_core_sdk_data.pull_data.stream_data（抖音推流地址最常驻于此）
    structured = _extract_streams_structured(render)
    if structured:
        return structured
    # 2) 兜底：递归收集所有字符串，反转义 \/ 后正则匹配（覆盖编码差异）
    strings: list[str] = []
    _walk_strings(render, strings)
    blob = "\n".join(strings).replace("\\/", "/")

    flv_urls, hls_urls = set(), set()
    for u in FLV_RE.findall(blob):
        if PULL_HOST_RE.search(u) or "douyin" in u:
            flv_urls.add(u.split("?")[0] if "flv?" not in u else u)
    for u in HLS_RE.findall(blob):
        if "douyin" in u or PULL_HOST_RE.search(u):
            hls_urls.add(u)

    streams = []
    for u in sorted(flv_urls):
        streams.append({"url": u, "type": "flv"})
    for u in sorted(hls_urls):
        streams.append({"url": u, "type": "hls"})
    return streams


def _extract_meta(render: dict):
    blob = json.dumps(render, ensure_ascii=False)
    name = ""
    mm = NICK_RE.search(blob)
    if mm:
        name = mm.group(1)
    room_id = ""
    rm = ROOMID_RE.search(blob)
    if rm:
        room_id = rm.group(1)
    return name, room_id


def _extract_web_rid(url: str) -> str:
    m = re.search(r"live\.douyin\.com/([0-9A-Za-z_]+)", url)
    return m.group(1) if m else ""


# ---------------- webcast/web/enter 回退路径 ----------------

def _ensure_ttwid(session, cookie: str) -> str:
    """确保请求带 ttwid cookie：用户已填则优先，否则自动向抖音首页索取。"""
    if cookie and "ttwid" in cookie:
        return cookie
    cookies = session.cookies.get_dict()
    if cookies.get("ttwid"):
        return "; ".join(f"{k}={v}" for k, v in cookies.items())
    try:
        r = session.get("https://live.douyin.com/", timeout=15)
        c = session.cookies.get_dict()
        if c.get("ttwid"):
            return "; ".join(f"{k}={v}" for k, v in c.items())
    except Exception:
        pass
    # 索要失败：沿用公开项目使用的样例 ttwid（尽力而为）
    return cookie or "ttwid=1%7C2iDIYVmjzMcpZ20fcaFde0VghXAA3NaNXE_SLR68IyE%7C1761045455%7Cab35197d5cfb21df6cbb2fa7ef1c9262206b062c315b9d04da746d0b37dfbc7d"


def _resolve_via_enter(page_url: str, session, cookie: str) -> dict:
    """回退路径：webcast/room/web/enter（真实 a_bogus 签名）。"""
    from ab_sign import get_a_bogus  # 局部导入，避免循环

    web_rid = _extract_web_rid(page_url)
    if not web_rid:
        raise RuntimeError("无法从链接提取房间号")

    ua = session.headers.get("User-Agent", DEFAULT_UA)
    ttwid_cookie = _ensure_ttwid(session, cookie)
    if ttwid_cookie:
        session.headers["Cookie"] = ttwid_cookie

    params = {
        "aid": "6383",
        "app_name": "douyin_web",
        "live_id": "1",
        "device_platform": "web",
        "language": "zh-CN",
        "browser_language": "zh-CN",
        "browser_platform": "Win32",
        "browser_name": "Chrome",
        "browser_version": "116.0.0.0",
        "web_rid": web_rid,
        "msToken": "",
    }
    a_bogus = get_a_bogus(ENTER_API, params, None, ua)
    if not a_bogus:
        raise RuntimeError("a_bogus 签名为空")
    params["a_bogus"] = a_bogus

    try:
        r = session.get(ENTER_API, params=params, timeout=20)
        r.raise_for_status()
        data = r.json()
    except Exception as e:
        raise RuntimeError(f"webcast enter 请求失败: {e}")

    try:
        d = data["data"]["data"][0]
    except Exception:
        raise RuntimeError(f"webcast enter 响应异常: {str(data)[:200]}")

    # 主播名
    name = ""
    try:
        name = d.get("owner", {}).get("nickname", "")
    except Exception:
        pass

    # 直播状态：status==2 表示直播中（0 未开播 / 2 直播中）
    try:
        if d.get("status") not in (None, 2):
            raise RuntimeError("主播当前未开播（webcast status=%s）" % d.get("status"))
    except RuntimeError:
        raise
    except Exception:
        pass

    streams: list = []
    try:
        su = d.get("stream_url", {})
        sdk = su.get("live_core_sdk_data", {})
        if sdk:
            streams = _parse_stream_data(sdk.get("pull_data", {}).get("stream_data"))
        if not streams:
            # 变体：pull_datas 结构
            pd = su.get("pull_datas") or {}
            for k, v in pd.items():
                if isinstance(v, dict) and v.get("stream_data"):
                    streams = _parse_stream_data(v["stream_data"])
                    if streams:
                        break
        if not streams:
            # 变体：flv_pull_url / hls_pull_url_map 直取
            flv_map = su.get("flv_pull_url") or {}
            if isinstance(flv_map, dict):
                for u in flv_map.values():
                    if u:
                        streams.append({"url": u, "type": "flv"})
            hls_map = su.get("hls_pull_url_map") or {}
            if isinstance(hls_map, dict):
                for u in hls_map.values():
                    if u:
                        streams.append({"url": u, "type": "hls"})
    except Exception:
        pass

    if not streams:
        raise RuntimeError("webcast enter 未返回可用流地址（可能签名被风控或房间未开播）")

    return {
        "room_id": web_rid,
        "web_rid": web_rid,
        "name": name or web_rid,
        "streams": streams,
        "source": "webcast_enter",
    }


# ---------------- 主入口 ----------------

def resolve(url: str, cookie: str = "", proxy: str = "") -> Optional[dict]:
    """解析直播间，返回推流信息。失败抛 RuntimeError（原因给 UI 展示）。"""
    _require_requests()
    page_url = normalize_room_input(url)
    if not page_url:
        raise RuntimeError("链接为空，请填写直播间链接")

    session = requests.Session()
    session.headers.update({
        "User-Agent": DEFAULT_UA,
        "Accept-Language": "zh-CN,zh;q=0.9",
        "Referer": "https://live.douyin.com/",
    })
    if cookie:
        session.headers["Cookie"] = cookie
    if proxy:
        session.proxies = {"http": proxy, "https": proxy}

    # ---- 路径 1：RENDER_DATA 直采 ----
    render_err = ""
    try:
        resp = session.get(page_url, timeout=20)
        resp.raise_for_status()
        html = resp.text
        render = _decode_render_data(html)
        if render:
            streams = _extract_streams(render)
            if streams:
                name, room_id = _extract_meta(render)
                web_rid = _extract_web_rid(page_url) or room_id
                return {
                    "room_id": room_id,
                    "web_rid": web_rid,
                    "name": name or web_rid,
                    "streams": streams,
                    "source": "render_data",
                }
            render_err = "RENDER_DATA 已加载但未提取到推流地址"
        else:
            render_err = "未从直播页找到 RENDER_DATA"
    except Exception as e:
        render_err = f"抓取直播页失败: {e}"

    # ---- 路径 2：webcast/web/enter（真实 a_bogus） ----
    try:
        return _resolve_via_enter(page_url, session, cookie)
    except Exception as e:
        raise RuntimeError(f"{render_err}；webcast 接口回退也失败: {e}")


if __name__ == "__main__":
    import sys
    u = sys.argv[1] if len(sys.argv) > 1 else input("直播间链接: ").strip()
    try:
        res = resolve(u)
        print(json.dumps(res, ensure_ascii=False, indent=2))
    except Exception as e:
        print("解析失败:", e)
