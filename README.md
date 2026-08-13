# DouyinRecorder · 抖音直播录屏工具 v3.8

本地化、自包含的抖音直播录制桌面工具。填入直播间链接即可监控，**开播自动录制、断流重连、智能识别（站立才录/坐下停）**，录制完成后自动合并为单个文件，并支持转码、合并、剪辑与实时预览。

## 功能
- 直播间解析：优先从直播页 `RENDER_DATA` 直采推流地址（flv / hls），无需签名；可选回退 `webcast/room/web/enter`。
- 在线自动录制：未开播按间隔检测，开播即录；断流自动重连。
- 智能识别（v3.8）：`posture` 单帧边缘分带检测——**站立（露腿/站直）才录，坐下/静止即停**；`motion` 运动触发；`person` 有人即录。滞后缓冲避免抖动误停。
- 分段自动合并：每段写入 `.segments/` 临时分片，段落结束自动 `concat` 合并为单个完整文件，便于人工裁剪。
- 转码 / 合并：录制后可自动转码（流拷贝 / 重编码），软件内可手动合并多个视频。
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

## 打包为 exe
```bash
pip install pyinstaller
# 把 ffmpeg.exe 放到本目录（或让它首次运行自动下载）
pyinstaller build.spec --noconfirm
# 产物在 dist/DouyinRecorder/，打包：
cd dist && powershell Compress-Archive DouyinRecorder DouyinRecorder_win64_v3.8.zip
```
解压后双击 `DouyinRecorder.exe` 即可使用（自包含 ffmpeg 与 Web UI）。

## 关于解析与 a_bogus
抖音的 `a_bogus` 是随时间演化的栈式 VM 签名，无法静态固化。**本工具主路径直接解析直播页 `RENDER_DATA` 获取流地址，通常无需 `a_bogus`**。
若你想走 `webcast/room/web/enter` 接口，请把当前有效的 `a_bogus` 实现替换进 `ab_sign.get_a_bogus`（保持函数签名一致），可参考 MIT 项目 `ihmily/DouyinLiveRecorder` 的 `ab_sign.py`。

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
MIT。a_bogus 思路改编自 DouyinLiveRecorder (ihmily, MIT)。
