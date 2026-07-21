# 抖音直播录屏（Douyin Recorder）

类 Olived 的抖音直播录制桌面程序。系统托盘常驻、后台静默运行，支持自动转码、flv 分片合并、
批量房间号、主播坐立检测（站立/有动作续录，坐下无动作停录）、已完成录屏预览剪辑与开机自启。

基于 **Tauri (Rust + Web)** 构建，常驻内存与 CPU/GPU 占用低；UI 使用 Vue3 + Tailwind，复刻 Olived 的
现代精致风格。

## 功能一览

- 系统托盘常驻，启动后无可见窗口（后台静默）
- 混合录制：优先抓取直播真实流地址 (flv 分片)，失败回退窗口/屏幕捕获
- 录制结束后自动合并分片并转出 mp4/mkv/mov/webm 等格式
- 批量导入房间号列表，逐个或一键监控全部
- 主播坐立状态检测（OpenCV 轻量模型）：站立/小幅动作 → 续录；坐下且无动作 → 防抖后停录
- 已完成录屏库：多视频合并、单文件转码、**时间轴预览剪辑（多段选取合并）**
- 开机自启开关

## 技术架构

```
Tauri(Rust)  ── 编排 ffmpeg ──► 录制 / 转码 / 合并
   │
   ├─ 检测器进程：ffmpeg(低清低帧) ─► python detector.py(OpenCV) ─► JSON 事件
   │
   └─ Vue3 + Tailwind 前端（精致的 Olived 风格 UI）
```

- `src-tauri/src/recorder.rs` 录制引擎（流优先 / 屏幕回退）
- `src-tauri/src/transcode.rs` 转码 / flv 合并 / 多视频合并 / 片段剪辑
- `src-tauri/src/detector.rs` 启动 ffmpeg→python 检测管线
- `src-tauri/src/logic.rs` 坐立状态机（防抖）
- `src-tauri/python/detector.py` OpenCV 坐立/动作检测
- `src/` Vue3 前端

## 开发 / 构建

> ⚠️ 由于 Tauri/WebView2 在 Windows 上构建最稳妥，最终的 `.exe` 需在 **Windows** 环境产出
> （本地 Windows 或下方 GitHub Actions）。本仓库已备好全部构建配置。

### 本地（Windows）一键出包

```bash
# 安装依赖：Node 20+、Rust 稳定版、ffmpeg(已加入 PATH)、Python 3.11+
pnpm install
pnpm tauri build
# 产物：src-tauri/target/release/bundle/nsis/*.exe （安装包）
```

### 检测功能所需的 Python + OpenCV

- **开发 / 系统已有 Python**：无需额外操作，程序会自动探测 `python` / `python3`。
  首次使用检测前请安装：`pip install -r src-tauri/python/requirements.txt`
- **发布版开箱即用**：CI 会把嵌入式 Python + OpenCV 打包进安装目录的 `python/`，
  程序优先使用该捆绑解释器，无需用户单独安装。

### GitHub Actions（自动出 exe）

推送 `v*` 标签或手动触发 `.github/workflows/build.yml`，
在 `windows-latest` 上自动构建并上传 NSIS 安装包作为 Artifact / Release。

## 使用说明

1. 安装并启动程序，它会在系统托盘常驻（默认隐藏主窗口）。
2. 「房间管理」添加房间号（可批量粘贴，支持换行/逗号/空格分隔）。
   - 若已获取直播流地址 (flv/hls)，在编辑中粘贴，录制将直接抓取流（最贴近 flv 分片合并需求）。
   - 未填流地址时自动回退为屏幕捕获（在设置中选择捕获源，如 `desktop` 或 `title=窗口标题`）。
3. 点击「开始监控」：主播站立/有动作时自动开始录制，坐下且无动作数秒后自动停止。
   或「立即录制」手动控制。
4. 「已完成录屏」可合并多个视频、转码格式，或「预览剪辑」用时间轴选取多段合并导出。
5. 设置中可调整输出格式、分片时长、检测灵敏度、坐下停录延迟与开机自启。

## 已知限制

- 抖音直播流地址带签名/反爬，自动解析未内置；当前需在房间设置中手动粘贴流地址。
- 坐立判定基于人脸占比 + 动作强度，极端机位可能需微调灵敏度。
