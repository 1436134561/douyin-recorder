# DouyinRecorder · 抖音直播录屏工具 v3.9

本地化、自包含的抖音直播录制桌面工具。填入直播间链接即可监控，**开播自动录制、断流重连、智能识别（站立才录/坐下停）**，支持转码、合并、剪辑与实时预览。

## 功能
- 直播间解析：优先从直播页 `RENDER_DATA` 直采推流地址（flv / hls）；失败自动回退 `webcast/room/web/enter` 接口（**真实 a_bogus 签名**，ttwid 自动索取）。
- 在线自动录制：未开播按间隔检测，开播即录；断流自动重连。
- 智能识别（v3.8 算法保留）：`posture` 单帧边缘分带检测——**站立（露腿/站直）才录，坐下/静止即停**；`motion` 运动触发；`person` 有人即录。滞后缓冲避免抖动误停。
- 分段直出文件：每段录制直接落输出目录 `{名称}_{时间}.0000.flv`、`.0001.flv`……**边录边出**，不做自动合并（保留 v3.7 文件行为）；软件内提供「合并已完成录屏」手动合并。
- 转码：录制后可自动转码（流拷贝 / 重编码），逐段进行。
- 实时预览：监控卡片一键低画质查看当前直播画面。
- 已完成录屏：列出输出目录视频，支持 HTML5 播放与按时间段剪辑导出。
- 系统托盘常驻、开机自启动、低资源模式。

## 运行（开发模式）
```bash
pip install -r requirements.txt
python main.py
```
依赖 WebView2（Win10/11 自带）；不可用自动降级到 tkinter GUI。
首次运行若目录下无 `ffmpeg.exe`，程序会尝试自动下载；也可手动放置 `ffmpeg.exe` 到程序目录。

## 打包为 exe（PyInstaller onedir）
```bash
pip install pyinstaller
# 把 ffmpeg.exe 放到本目录（或让它首次运行自动下载）
pyinstaller build.spec --noconfirm --distpath dist_v39 --workpath build_v39
```
产物在 `dist_v39/DouyinRecorder/`（自包含 ffmpeg 与 Web UI）。

## 打包为安装包（NSIS，与原版 douyin-recorder-setup 一致）
```bash
# 先按上一步生成 dist_v39/DouyinRecorder，然后：
makensis.exe douyin_setup.nsi
# 产物：douyin-recorder-setup-v3.9.exe（按用户安装到 %LOCALAPPDATA%\Programs\DouyinRecorder）
```
GitHub 仓库已配置 Actions（`.github/workflows/build.yml`）：推送 `main` 或打 `v*` 标签即自动构建 NSIS 安装包并上传 Release。

## 关于解析与 a_bogus
`ab_sign.py` 内置**真实 a_bogus 签名**（SM3 + RC4，纯 Python，无 Node 依赖），改编自 MIT 项目 `ihmily/DouyinLiveRecorder` 的 `ab_sign.py`（Copyright 2025 Hmily，已保留版权声明）。解析流程：取 `ttwid` Cookie → 调 `webcast/room/web/enter`（带 a_bogus 签名）→ 从 `live_core_sdk_data.pull_data.stream_data` 解析多画质 flv/hls 地址。抖音签名算法会不定期更新，若出现普遍解析失败，需同步上游新版算法。

## 智能识别参数（config.json）
| 参数 | 默认 | 说明 |
|---|---|---|
| smart_mode | posture | posture / motion / person |
| smart_interval | 1.0 | 采样间隔(秒)，0.5~5 |
| smart_posture_span | 0.60 | 身体边缘延伸到画面的比例阈值 |
| smart_posture_bottom_ratio | 0.45 | 底部/顶部边缘强度比阈值 |
| smart_posture_motion | 8.0 | 跳舞兜底：变化比例阈值(%) |
| smart_posture_keep | 5 | 站立保持记忆(秒)，防站立静态误停 |

## 与游戏反作弊
软件仅本地从直播流 URL 拉流并 `-c copy` 写文件，**不注入/不读游戏内存/不模拟键鼠/不 hook/不改包**，与游戏进程完全隔离，不会导致 EAC/BattlEye/VAC 等封号。唯一风险是平台 ToS 层面（部分赛事禁直播录屏）。

## 许可
MIT。a_bogus 实现改编自 DouyinLiveRecorder (ihmily, MIT)。
