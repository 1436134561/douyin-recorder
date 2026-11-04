use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use anyhow::{anyhow, Result};
use serde_json::json;
use tauri::{AppHandle, Emitter, Runtime};

use crate::config::{AppConfig, RoomConfig, VIDEO_EXTS};
use crate::ffmpeg::ffmpeg_executable;
use crate::state::AppState;
use crate::transcode;
use crate::types::RecordingInfo;

/// 一次录制会话
pub struct RecordingSession {
    #[allow(dead_code)]
    pub room_id: String,
    pub mode: String,
    pub work_dir: PathBuf,
    pub process: Option<std::process::Child>,
    #[allow(dead_code)]
    pub started_at: i64,
    /// 下一个分片序号（自动重连时续号，避免覆盖已有分片）
    #[allow(dead_code)]
    pub seg_next: u32,
    /// 已自动重连次数
    pub retries: u32,
    /// ffmpeg stderr 落盘位置（不用管道，避免管道写满把录制阻塞住）
    #[allow(dead_code)]
    pub log_path: PathBuf,
    /// 本次录制实际输出的分辨率（从 ffmpeg 日志解析，如 `1080x1920`）
    /// 用于确认没有被降级 —— 前端会展示，也方便用户自查
    pub resolution: Option<String>,
}

/// 单次录制最多自动重连次数
pub const MAX_RECONNECT: u32 = 5;

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 清洗房间 ID/文件名，只保留文件系统安全的字符
pub fn sanitize_room_id(room_id: &str) -> String {
    room_id
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect()
}

/// 格式化时间戳为 `2026-07-23_18-52-34`（本地时间，使用 chrono）
fn fmt_ts(ts: i64) -> String {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%d_%H-%M-%S").to_string())
        .unwrap_or_else(|| format!("{}", ts))
}

/// 在 Windows 上隐藏子进程的控制台窗口
fn spawn_hidden(cmd: &mut Command) -> Result<std::process::Child> {
    #[cfg(windows)]
    let child = cmd
        .creation_flags(0x08000000)
        .spawn()
        .map_err(|e| anyhow!("进程启动失败: {}", e))?;
    #[cfg(not(windows))]
    let child = cmd
        .spawn()
        .map_err(|e| anyhow!("进程启动失败: {}", e))?;
    Ok(child)
}

/// 根据配置与房间决定录制模式
fn decide_mode(cfg: &AppConfig, room: Option<&RoomConfig>) -> &'static str {
    let has_url = room.and_then(|r| r.stream_url.clone()).is_some();
    let want_stream = cfg.capture_mode != "screen";
    if has_url && want_stream {
        "stream"
    } else {
        "screen"
    }
}

/// 解析检测器取流参数 (source, mode)
pub fn resolve_source(room_id: &str, cfg: &AppConfig, room: Option<&RoomConfig>) -> (String, String) {
    let mode = decide_mode(cfg, room);
    let source = if mode == "stream" {
        room.and_then(|r| r.stream_url.clone())
            .or_else(|| resolve_stream_url(room_id))
            .unwrap_or_default()
    } else {
        cfg.screen_source.clone().unwrap_or_else(|| "desktop".into())
    };
    (source, mode.to_string())
}

fn resolve_stream_url(_room_id: &str) -> Option<String> {
    None
}

/// 开始录制
pub fn begin_recording<R: Runtime>(
    room_id: &str,
    spawn_detector: bool,
    app: &AppHandle<R>,
    state: &Arc<Mutex<AppState>>,
) -> Result<()> {
    let (cfg, room) = {
        let st = state.lock().unwrap();
        let cfg = st.config.clone();
        let room = st.config.rooms.iter().find(|r| r.id == room_id).cloned();
        (cfg, room)
    };
    if state.lock().unwrap().recordings.contains_key(room_id) {
        return Ok(());
    }

    let session = start_ffmpeg(room_id, &cfg, room.as_ref())?;
    let resolution = session.resolution.clone();
    if let Some(ref r) = resolution {
        eprintln!("[recorder] 实际录制分辨率 = {}", r);
    }
    state
        .lock()
        .unwrap()
        .recordings
        .insert(room_id.to_string(), session);

    // 启动后台看护：5 秒一次检查 ffmpeg 进程状态 + 文件增长
    spawn_recording_watcher(
        room_id.to_string(),
        app.clone(),
        state.clone(),
    );

    if spawn_detector && cfg.detect_enabled {
        let (source, mode) = resolve_source(room_id, &cfg, room.as_ref());
        match crate::detector::spawn_detector(
            room_id.to_string(),
            source,
            mode,
            cfg.sensitivity,
            cfg.sit_stop_seconds,
            false,
            cfg.python_path.clone(),
            app.clone(),
            state.clone(),
        ) {
            Ok(h) => {
                state.lock().unwrap().detectors.insert(room_id.to_string(), h);
            }
            Err(e) => eprintln!("检测器启动失败: {}", e),
        }
    }

    let _ = app.emit(
        "recording_started",
        json!({ "room_id": room_id, "resolution": resolution }),
    );
    Ok(())
}

/// 后台看护线程：每 5 秒检查 ffmpeg 是否还活着 + work_dir 文件是否在增长
///
/// 故障处理顺序（这是「录屏十几兆就不录了」的核心修复）：
/// 1. 检测到 ffmpeg 退出 / 180 秒无增长 → **先自动重连**（重新解析流地址，续写同一工作目录）
/// 2. 重连次数用尽（MAX_RECONNECT）才落盘已录内容并报告失败
///
/// 看护线程会一直运行直到本房间的 session 被移除（用户停止 / 检测器停 / 自动清理）
fn spawn_recording_watcher<R: Runtime>(
    room_id: String,
    app: AppHandle<R>,
    state: Arc<Mutex<AppState>>,
) {
    std::thread::spawn(move || {
        let mut last_total_bytes: u64 = 0;
        let mut last_growth = std::time::Instant::now();
        // 放宽阈值：慢速直播流 60s 可能还没有新关键帧
        const STALE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);
        const NO_DATA_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

        loop {
            std::thread::sleep(std::time::Duration::from_secs(5));

            // 1. 本房间是否还在录制？快速锁定以调用 try_wait
            let (still_recording, work_dir_opt, child_alive, mode, retries) = {
                let mut st = state.lock().unwrap();
                match st.recordings.get_mut(&room_id) {
                    Some(session) => {
                        let alive = match session.process.as_mut() {
                            Some(c) => c.try_wait().ok().map(|s| s.is_none()).unwrap_or(false),
                            None => false,
                        };
                        (
                            true,
                            Some(session.work_dir.clone()),
                            alive,
                            session.mode.clone(),
                            session.retries,
                        )
                    }
                    None => (false, None, false, String::new(), 0),
                }
            };

            if !still_recording {
                break;
            }

            // 2. 视频分片增长情况（只统计视频文件，日志文件增长不算「还在录」）
            let total = work_dir_opt
                .as_deref()
                .map(video_bytes_in)
                .unwrap_or(0);

            if total > last_total_bytes {
                last_total_bytes = total;
                last_growth = std::time::Instant::now();
            }

            let no_data = total == 0 && last_growth.elapsed() > NO_DATA_TIMEOUT;
            let stalled = total > 0 && last_growth.elapsed() > STALE_TIMEOUT;
            if child_alive && !no_data && !stalled {
                continue;
            }

            let reason = if !child_alive {
                "录制进程已退出（流地址失效 / 网络断开 / 主播下播）"
            } else if no_data {
                "录制启动后 90 秒内未收到任何数据（流地址可能无效）"
            } else {
                "录制文件 180 秒无增长（流已中断）"
            };

            // 3. 流模式优先自动重连，而不是直接放弃
            if mode == "stream" && retries < MAX_RECONNECT {
                let _ = app.emit(
                    "recording_reconnecting",
                    json!({
                        "room_id": room_id,
                        "attempt": retries + 1,
                        "max": MAX_RECONNECT,
                        "reason": reason,
                    }),
                );
                match reconnect_recording(&room_id, &app, &state) {
                    Ok(()) => {
                        eprintln!("[recorder] {} 第 {} 次重连成功，继续录制", room_id, retries + 1);
                        last_total_bytes = 0;
                        last_growth = std::time::Instant::now();
                        continue;
                    }
                    Err(e) => {
                        eprintln!("[recorder] {} 第 {} 次重连失败: {}", room_id, retries + 1, e);
                    }
                }
            }

            // 4. 重连无望 → 保存已录内容并报告失败
            finalize_partial_or_cleanup(&room_id, reason, &app, &state);
            break;
        }
    });
}

/// 自动重连：重启 ffmpeg，续写同一个工作目录（分片序号接着编，不覆盖已录分片）
///
/// 抖音 FLV 地址时效通常只有几分钟，直接让 ffmpeg `-reconnect` 重试旧地址是无效的，
/// 必须重新解析出新的流地址再拉起进程。
fn reconnect_recording<R: Runtime>(
    room_id: &str,
    app: &AppHandle<R>,
    state: &Arc<Mutex<AppState>>,
) -> Result<()> {
    // 1. 杀掉旧进程，取出工作目录 / 续号 / 重试计数
    let (cfg, work, seg_start, retries) = {
        let mut st = state.lock().unwrap();
        let cfg = st.config.clone();
        let session = st
            .recordings
            .get_mut(room_id)
            .ok_or_else(|| anyhow!("录制会话已不存在"))?;
        if let Some(mut c) = session.process.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        let next = next_segment_index(&session.work_dir);
        (
            cfg,
            session.work_dir.clone(),
            next,
            session.retries + 1,
        )
    };

    // 2. 重新解析流地址
    let page = format!("https://live.douyin.com/{}", room_id);
    let flv = resolve_flv_blocking(&page).map_err(|e| anyhow!("重新解析流地址失败: {}", e))?;

    // 3. 回写房间配置，下次手动录制直接可用
    {
        let mut st = state.lock().unwrap();
        if let Some(r) = st.config.rooms.iter_mut().find(|r| r.id == room_id) {
            r.stream_url = Some(flv.clone());
        }
        let _ = crate::config::save_config(&st.config);
    }

    // 4. 拉新 ffmpeg（同一工作目录 + 分片续号）
    let session = spawn_ffmpeg(
        room_id,
        &cfg,
        "stream",
        Some(flv.as_str()),
        &work,
        seg_start,
        retries,
    )?;

    // 5. 装回 state（期间用户可能已停止录制 → 必须收掉刚起的进程，别留孤儿）
    let resolution = session.resolution.clone();
    let mut st = state.lock().unwrap();
    match st.recordings.get_mut(room_id) {
        Some(slot) => {
            *slot = session;
        }
        None => {
            let mut orphan = session;
            if let Some(mut c) = orphan.process.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
            return Err(anyhow!("录制已被用户停止"));
        }
    }
    drop(st);

    let _ = app.emit(
        "recording_reconnected",
        json!({ "room_id": room_id, "attempt": retries, "resolution": resolution }),
    );
    Ok(())
}

/// 在普通线程里同步解析直播流地址
///
/// 看护线程是 std::thread，没有 tokio 上下文，不能直接用 tauri::async_runtime::block_on
/// （可能 panic / 依赖调用线程）。这里自建一个专用 runtime，创建一次后复用。
fn resolve_flv_blocking(page: &str) -> Result<String> {
    use once_cell::sync::Lazy;
    static RT: Lazy<tokio::runtime::Runtime> = Lazy::new(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .thread_name("stream-resolver")
            .build()
            .expect("create stream resolver runtime")
    });
    let resolved = RT.block_on(crate::stream_resolver::resolve(page))?;
    eprintln!(
        "[recorder] 重连解析画质 = {}（{}）",
        resolved.quality,
        crate::stream_resolver::quality_hint(&resolved.quality)
    );
    Ok(resolved.flv)
}

/// 统计目录内视频文件总字节数（不含日志/临时文件）
fn video_bytes_in(dir: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            let ext = p
                .extension()
                .and_then(|x| x.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if matches!(ext.as_str(), "flv" | "mp4" | "ts" | "mkv") {
                if let Ok(m) = std::fs::metadata(&p) {
                    total += m.len();
                }
            }
        }
    }
    total
}

/// 解析 `seg_00012.flv` 的分片序号
fn seg_number(p: &Path) -> u32 {
    p.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_prefix("seg_"))
        .and_then(|rest| rest.split('.').next())
        .and_then(|n| n.parse::<u32>().ok())
        .unwrap_or(0)
}

/// 计算下一个可用的分片序号（max + 1）
fn next_segment_index(dir: &Path) -> u32 {
    let mut max = 0u32;
    let mut found = false;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "flv").unwrap_or(false) {
                let n = seg_number(&p);
                if !found || n > max {
                    max = n;
                    found = true;
                }
            }
        }
    }
    if found {
        max.saturating_add(1)
    } else {
        0
    }
}

/// 看护触发时调用：先尝试用 stop_and_finalize 保存部分分片（不丢数据），
/// 之后无论成功失败都发 recording_failed 事件通知前端
fn finalize_partial_or_cleanup<R: Runtime>(
    room_id: &str,
    reason: &str,
    app: &AppHandle<R>,
    state: &Arc<Mutex<AppState>>,
) {
    // 先把正在进行的 process 杀掉（避免 stop_and_finalize 内部 kill 已经死掉的进程出错）
    let has_session = {
        let mut st = state.lock().unwrap();
        if let Some(session) = st.recordings.get_mut(room_id) {
            if let Some(mut c) = session.process.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
            true
        } else {
            false
        }
    };

    if has_session {
        // 尝试保存分片为最终 mp4
        if let Err(e) = stop_and_finalize(room_id, false, app, state) {
            eprintln!("看护清理时保存部分数据失败：{}", e);
        }
    }

    // 通知前端：清理状态 + 失败原因
    let _ = app.emit(
        "recording_failed",
        json!({
            "room_id": room_id,
            "reason": reason,
        }),
    );
    let _ = app.emit(
        "recording_stopped",
        json!({ "room_id": room_id, "id": "", "failed": true }),
    );
}

/// 启动 ffmpeg 录制子进程（新会话：创建独立工作目录，分片从 0 开始）
fn start_ffmpeg(
    room_id: &str,
    cfg: &AppConfig,
    room: Option<&RoomConfig>,
) -> Result<RecordingSession> {
    std::fs::create_dir_all(&cfg.output_dir)?;
    // 关键修复：每次会话用独立工作目录。
    // 旧实现固定用 `._work_<room>`，上一次失败的残留分片（含 merged.flv）
    // 会被下一次 `stop_and_finalize` 的 gather_segments 一起合并进去 → 视频内容重复。
    let work = cfg
        .output_dir
        .join(format!("._work_{}@{}", sanitize_room_id(room_id), now_ts()));
    std::fs::create_dir_all(&work)?;

    let mode = decide_mode(cfg, room);
    let url = if mode == "stream" {
        Some(
            room.and_then(|r| r.stream_url.clone())
                .or_else(|| resolve_stream_url(room_id))
                .ok_or_else(|| anyhow!("房间 {} 未配置直播流地址", room_id))?,
        )
    } else {
        None
    };

    spawn_ffmpeg(room_id, cfg, mode, url.as_deref(), &work, 0, 0)
}

/// 在指定工作目录里拉起 ffmpeg（供首次启动与自动重连复用）
///
/// `seg_start`：分片起始序号（重连时续号）
/// `retries`：已重连次数（写回 session 供看护线程判断额度）
fn spawn_ffmpeg(
    room_id: &str,
    cfg: &AppConfig,
    mode: &str,
    url: Option<&str>,
    work: &Path,
    seg_start: u32,
    retries: u32,
) -> Result<RecordingSession> {
    let ffmpeg_bin = ffmpeg_executable();
    // 关键修复：把 child 工作目录设为 ffmpeg.exe 所在目录。
    // mingw 编译的 ffmpeg（gyan.dev essentials / BtbN nightly）依赖同目录的
    // gnuwin32 runtime DLL。如果 spawn 时 cwd 不在 ffmpeg 同目录，
    // DLL 加载阶段就异常退出（exit code 0xABAFB008 这种异常值，stderr 只有 banner）。
    let ffmpeg_dir = Path::new(&ffmpeg_bin)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    // 关键修复：stderr 不再用管道。
    // 旧实现 `Cmd::stderr(Stdio::piped())` 后 5 秒就 drop 掉读端，而 ffmpeg 用
    // `-v info` 每 0.5 秒写一行进度；管道缓冲（约 4KB）写满后 ffmpeg 会**阻塞在写日志上**，
    // 编码随之停住 —— 表现就是「录了十几兆就不动了」。
    // 现在把 stderr 直接重定向到日志文件，永远不会阻塞，还能用来定位失败原因。
    let log_path = work.join("ffmpeg.log");
    let log_file = File::create(&log_path)
        .map_err(|e| anyhow!("创建 ffmpeg 日志失败（{}）: {}", log_path.display(), e))?;

    let mut cmd = Command::new(&ffmpeg_bin);
    cmd.current_dir(&ffmpeg_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log_file))
        .arg("-y")
        .args(["-hide_banner", "-v", "info"]);

    if mode == "stream" {
        let url = url.ok_or_else(|| anyhow!("房间 {} 缺少直播流地址", room_id))?;
        let seg_time = (cfg.segment_minutes.max(1) * 60).to_string();
        let seg_start_s = seg_start.to_string();
        let out_tmpl = work.join("seg_%05d.flv");
        // ↓↓ 断流重连参数必须放在 -i 之前，否则 ffmpeg 会当成输出选项直接忽略
        //    （这正是「十几兆就不录了」的元凶：旧代码把它们写在了 -i 之后）
        cmd.args([
            "-rw_timeout",
            "15000000", // 读超时 15s：卡死能主动报错而不是无限挂住
            "-reconnect",
            "1",
            "-reconnect_streamed",
            "1",
            "-reconnect_at_eof",
            "1", // CDN 主动 EOF 也要重连（直播流常见）
            "-reconnect_delay_max",
            "10",
        ])
        .arg("-i")
        .arg(url)
        .args(["-c", "copy", "-f", "segment", "-segment_time"])
        .arg(seg_time)
        .args(["-segment_format", "flv", "-reset_timestamps", "1"])
        .arg("-segment_start_number")
        .arg(seg_start_s)
        .arg(out_tmpl.as_os_str());
    } else {
        let src = cfg.screen_source.clone().unwrap_or_else(|| "desktop".into());
        let out_file = work.join("rec.mp4");
        cmd.args(["-f", "gdigrab", "-framerate", "30"])
            .arg("-i")
            .arg(&src)
            .args([
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
                "-f",
                "mp4",
            ])
            .arg(out_file.as_os_str());
    }

    let mut child = spawn_hidden(&mut cmd).map_err(|e| anyhow!("ffmpeg 录制启动失败: {}", e))?;

    // 启动后等待 ffmpeg 连接流（流地址解析/握手可能慢）
    std::thread::sleep(std::time::Duration::from_millis(2000));
    if let Ok(Some(status)) = child.try_wait() {
        if !status.success() {
            let log = tail_log(&log_path, 1500);
            return Err(ffmpeg_exit_error(&log, status.code(), 2, mode));
        }
    }

    // 再等 3 秒检查分片是否开始增长（流地址过期时 ffmpeg 活着但收不到数据）
    std::thread::sleep(std::time::Duration::from_millis(3000));
    let has_data = video_bytes_in(work) > 0;
    if !has_data {
        if let Ok(Some(status)) = child.try_wait() {
            if !status.success() {
                let log = tail_log(&log_path, 1500);
                return Err(ffmpeg_exit_error(&log, status.code(), 5, mode));
            }
        }
        let log = tail_log(&log_path, 1500);
        let _ = child.kill();
        let _ = child.wait();
        return Err(anyhow!(
            "开始录制 5 秒后仍未收到数据。\n\
             可能原因：\n\
             ① 流地址已过期（抖音 FLV 地址时效性 ~几分钟，下次会自动重新解析）\n\
             ② 直播间未在直播或被封禁\n\
             ③ 网络防火墙 / 代理问题\n\
             \n\
             请确认直播间正在直播，并重新开始录制。\n\
             \nffmpeg 日志尾部：\n{}",
            log
        ));
    }

    Ok(RecordingSession {
        room_id: room_id.to_string(),
        mode: mode.to_string(),
        work_dir: work.to_path_buf(),
        process: Some(child),
        started_at: now_ts(),
        seg_next: seg_start,
        retries,
        resolution: parse_resolution_from_log(&log_path),
        log_path,
    })
}

/// 读取日志文件尾部（供错误提示使用）
///
/// 取代旧的 `drain_stderr`：stderr 已改落盘，不再有管道排空/竞态问题。
fn tail_log(path: &Path, max_chars: usize) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = match File::open(path) {
        Ok(f) => f,
        Err(_) => return String::from("（无 ffmpeg 日志）"),
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(16 * 1024);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return String::from("（读取 ffmpeg 日志失败）");
    }
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    if buf.is_empty() {
        return String::from("（日志为空，可能 ffmpeg 被外力终止，如杀毒软件）");
    }
    let s = String::from_utf8_lossy(&buf).into_owned();
    let n = s.chars().count();
    if n > max_chars {
        s.chars().skip(n - max_chars).collect()
    } else {
        s
    }
}

/// 从 ffmpeg 日志里解析实际输出的分辨率（`Stream #0:0: Video: h264 ..., 1080x1920, ...`）
///
/// 为什么不用 ffprobe：发行包里只 sidecar 了 ffmpeg.exe，没有 ffprobe。
/// 而 `-c copy` 模式下 ffmpeg 会把输入流的真实分辨率打印到 stderr（现已落盘到 ffmpeg.log），
/// 直接解析这段日志即可拿到**实际**录制分辨率 —— 这正好可以用来验证没有被降级。
fn parse_resolution_from_log(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut f = File::open(path).ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;

    for line in s.lines() {
        let Some(idx) = line.find("Video:") else {
            continue;
        };
        // 只在 "Video:" 之后找第一处 NNNxNNN，避开 SAR/DAR 等无关数字
        let tail: Vec<char> = line[idx..].chars().collect();
        let mut i = 0;
        while i < tail.len() {
            if !tail[i].is_ascii_digit() {
                i += 1;
                continue;
            }
            let ws = i;
            let mut j = i;
            while j < tail.len() && tail[j].is_ascii_digit() {
                j += 1;
            }
            if j < tail.len() && tail[j] == 'x' {
                let mut k = j + 1;
                while k < tail.len() && tail[k].is_ascii_digit() {
                    k += 1;
                }
                let w: String = tail[ws..j].iter().collect();
                let h: String = tail[j + 1..k].iter().collect();
                // 合理分辨率范围，防止把 `5x5` 之类的时长/参数当成分辨率
                let valid = (2..=5).contains(&w.len())
                    && (2..=5).contains(&h.len())
                    && w.parse::<u32>().unwrap_or(0) >= 16
                    && h.parse::<u32>().unwrap_or(0) >= 16;
                if valid {
                    return Some(format!("{}x{}", w, h));
                }
            }
            i = j.max(i + 1);
        }
    }
    None
}

/// 生成 ffmpeg 退出错误（包含十六进制退出码 + stderr 尾部 + 分类提示）
///
/// 异常退出码识别（用户实测反馈后加入）：
/// - 0xABxxxxxx 模式（如 0xABAFB008）：ffmpeg 自己不会返回这种值，这是 Windows
///   调试堆填充模式 + 进程被外力 kill（杀毒软件 / Windows Defender / EDR / 调试器）后的特征。
///   stderr 只有 banner 没有错误消息也是同一信号。
/// - 高位 0xC000000x：Windows 异常码（如 0xC0000005 access violation / 0xC0000409 stack overflow）
/// - 其他常见：1（普通错误）、-1（无法获取）
fn ffmpeg_exit_error(stderr: &str, code: Option<i32>, waited_secs: u64, mode: &str) -> anyhow::Error {
    let c = code.unwrap_or(-1);
    let hex = if c >= 0 {
        format!("0x{:08X}", c as u32)
    } else {
        format!("0x{:08X} (signed: {})", c as u32, c)
    };

    // 异常退出码/空 stderr 检测 —— 强烈指向杀毒软件 / Windows Defender
    let abnormal_code = c == -1414549496  // 0xABAFB008
        || c == -1414549419              // 0xABABABAB
        || (c > 0 && (c as u32 & 0xFF000000) == 0xAB000000)
        || (c > 0 && (c as u32 & 0xFFFF0000) == 0xC0000000);
    let empty_stderr = stderr.is_empty()
        || stderr.contains("stderr 无输出")
        || stderr.contains("stderr 未捕获")
        || stderr.contains("日志为空");

    // 分类提示（按优先级：abnormal_code > empty_stderr > 关键词）
    // 关键：异常退出码（0xABxxxxxx 或 0xC000000x）单独就触发"疑似杀毒"提示
    // 不再要求 stderr 必须空 —— mingw ffmpeg 至少会打 banner，所以 stderr 不空但 exit code 异常
    // = 进程在 banner 后被外力 kill（典型 AV 启发式特征）
    let hint = if abnormal_code {
        "🚨 高度疑似被杀毒软件 / Windows Defender 拦截（99% 概率）：\n\
         · 异常退出码 0xABxxxxxx 是 Windows 调试堆填充 + 进程被外力 kill 的特征\n\
         · stderr 即使有内容也只有 banner，banner 后立刻退出 = 进程初始化阶段被杀\n\
         · mingw 编译的 ffmpeg.exe 常被 AV 启发式标记\n\
         · 修复（30 秒验证）：\n\
           1. 临时关 Windows Defender 实时保护（设置 → 病毒防护 → 管理设置 → 实时保护 关）\n\
           2. 再点一次立即录制\n\
           3. 如果成功 → 100% 确认是杀毒\n\
           4. 把 ffmpeg.exe 和安装目录加入白名单，再开实时保护\n\
         · 或直接在 Windows 安全中心 → 病毒防护 → 排除项 → 添加文件/文件夹"
    } else if empty_stderr {
        "🚨 疑似被杀毒软件 / Windows Defender 拦截：stderr 无输出说明 ffmpeg 进程被外力终止"
    } else if stderr.contains("403") || stderr.contains("Forbidden") {
        "（最可能：流地址已过期，HTTP 403。请重新开始录制，会自动重新解析）"
    } else if stderr.contains("Connection refused") || stderr.contains("timeout") || stderr.contains("timed out") {
        "（最可能：网络不可达 — 防火墙/代理/抖音风控）"
    } else if stderr.contains("404") || stderr.contains("Not Found") {
        "（最可能：流地址已失效 — 请重新开始录制自动重新解析）"
    } else if mode == "screen" {
        "（最可能：屏幕捕获源不可用 — 检查屏幕源设置）"
    } else {
        "（请确认主播正在直播，或参考下方 ffmpeg stderr）"
    };

    anyhow!(
        "ffmpeg 启动后{}秒内异常退出（exit code: {} / {}）{}\n\n\
         排查方向：\n\
         ① 流地址已过期（抖音 flv URL 几分钟就失效，重新开始录制会自动重新解析）\n\
         ② 主播未在直播或被风控拦截\n\
         ③ 杀毒软件 / Windows Defender 把 ffmpeg.exe 当作可疑进程杀掉（最常见）\n\
         ④ ffmpeg 找不到或缺少编解码器\n\
         \n\
         ffmpeg stderr 尾部：\n{}",
        waited_secs,
        c,
        hex,
        hint,
        stderr
    )
}

/// 收集有效分片文件（阈值 1KB：排除 0 字节空壳文件，保留任何有内容的数据）
///
/// 只收 `seg_*.flv`（流模式）/ `rec.mp4`（屏幕模式）：
/// 旧实现把工作目录下所有 .flv 都收进来，会误把 `merged.flv`（上次合并产物）
/// 也当成分片合并一遍 → 导出视频内容重复。
fn gather_segments(session: &RecordingSession) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    if session.mode == "stream" {
        if let Ok(entries) = std::fs::read_dir(&session.work_dir) {
            for e in entries.flatten() {
                let p = e.path();
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                if !name.starts_with("seg_") || !name.ends_with(".flv") {
                    continue;
                }
                if let Ok(meta) = std::fs::metadata(&p) {
                    if meta.len() > 1_024 {
                        files.push(p);
                    }
                }
            }
        }
        files.sort_by_key(|p| seg_number(p));
    } else {
        let f = session.work_dir.join("rec.mp4");
        if f.exists() {
            if let Ok(meta) = std::fs::metadata(&f) {
                if meta.len() > 1_024 {
                    files.push(f);
                }
            }
        }
    }
    files
}

/// 列出工作目录里的视频分片（供「等待中」恢复使用，不依赖 RecordingSession）
pub fn list_work_dir_segments(work_dir: &Path) -> Vec<PathBuf> {
    let mut segs: Vec<PathBuf> = Vec::new();
    let mut mp4: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(work_dir) {
        for e in entries.flatten() {
            let p = e.path();
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            let ok_size = std::fs::metadata(&p).map(|m| m.len() > 1_024).unwrap_or(false);
            if !ok_size || !p.is_file() {
                continue;
            }
            if name.starts_with("seg_") && name.ends_with(".flv") {
                segs.push(p);
            } else if name == "rec.mp4" {
                mp4.push(p);
            }
        }
    }
    if segs.is_empty() {
        segs = mp4;
    }
    segs.sort_by_key(|p| seg_number(p));
    segs
}

/// 把一个残留工作目录收尾成正式录像（合并 → 转码 → 落盘 → 清理工作目录）
///
/// 供「等待中」页面手动恢复调用。`room_label` 用于文件名（主播名或房间号）。
pub fn finalize_work_dir(
    work_dir: &Path,
    room_label: &str,
    cfg: &AppConfig,
    created_ts: i64,
) -> Result<RecordingInfo> {
    let files = list_work_dir_segments(work_dir);
    if files.is_empty() {
        return Err(anyhow!(
            "该工作目录里没有有效分片（目录：{}）",
            work_dir.display()
        ));
    }
    let merged = work_dir.join("merged.flv");
    if files.len() > 1 {
        transcode::merge_segments(&files, &merged)?;
    } else if let Some(f) = files.first() {
        std::fs::copy(f, &merged)?;
    }
    let merged_len = std::fs::metadata(&merged).map(|m| m.len()).unwrap_or(0);
    if merged_len == 0 {
        return Err(anyhow!("合并后无有效数据（原始分片仍在：{}）", work_dir.display()));
    }

    let effective_fmt = if cfg.auto_mp4 { "mp4" } else { cfg.output_format.as_str() };
    let ts = if created_ts > 0 { created_ts } else { now_ts() };
    let name = format!(
        "{}_{}.{}",
        sanitize_room_id(room_label),
        fmt_ts(ts),
        effective_fmt
    );
    let out_path = cfg.output_dir.join(&name);
    transcode::transcode(&merged, &out_path, effective_fmt).map_err(|e| {
        anyhow!(
            "转码失败: {}（原始分片保留在 {}）",
            e,
            work_dir.display()
        )
    })?;

    // 成功才清理工作目录
    let _ = std::fs::remove_dir_all(work_dir);

    let suffix = format!(".{}", effective_fmt);
    let id = name.strip_suffix(&suffix).unwrap_or(&name).to_string();
    Ok(RecordingInfo {
        id,
        room_id: room_label.to_string(),
        path: out_path.to_string_lossy().into(),
        size_bytes: std::fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0),
        duration_sec: 0.0,
        format: effective_fmt.to_string(),
        created_at: ts,
    })
}

/// 计算最终输出文件名（主播名_时间戳.扩展名）
fn build_final_name(cfg: &AppConfig, room_id: &str, ts: i64) -> String {
    let anchor_name = cfg
        .rooms
        .iter()
        .find(|r| r.id == room_id)
        .and_then(|r| r.name.clone())
        .filter(|s| !s.is_empty())
        .map(|s| sanitize_room_id(&s))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| sanitize_room_id(room_id));
    let effective_fmt = if cfg.auto_mp4 { "mp4" } else { &cfg.output_format };
    format!("{}_{}.{}", anchor_name, fmt_ts(ts), effective_fmt)
}

/// 停止录制并最终化
///
/// `keep_detector`：
/// - `false`：用户主动停止 → 同时销毁检测器（传统模式）
/// - `true`：检测器触发停止 → 保留检测器，等待下次循环触发（坐立检测循环模式）
///
/// 注意：`recordings.remove()` 在函数开头执行**就会清掉录制条目**，
/// 即使后续转码失败，状态也已经改变 → 必须发 recording_stopped 事件让前端刷新。
pub fn stop_and_finalize<R: Runtime>(
    room_id: &str,
    keep_detector: bool,
    app: &AppHandle<R>,
    state: &Arc<Mutex<AppState>>,
) -> Result<RecordingInfo> {
    let (session, det) = {
        let mut st = state.lock().unwrap();
        let s = st.recordings.remove(room_id);
        let d = if keep_detector { None } else { st.detectors.remove(room_id) };
        if !keep_detector {
            st.logic.remove(room_id);
        }
        (s, d)
    };
    if let Some(mut d) = det {
        d.stop();
    }
    let mut session = match session {
        Some(s) => s,
        None => {
            // 不在录制中。但前端 UI 可能认为在（状态漂移），仍然发出停止事件让它刷新
            let _ = app.emit(
                "recording_stopped",
                serde_json::json!({ "room_id": room_id, "id": "" }),
            );
            return Err(anyhow!("房间 {} 未在录制", room_id));
        }
    };

    // 停止 ffmpeg
    if let Some(mut c) = session.process.take() {
        let _ = c.kill();
        let _ = c.wait();
    }
    // 等待文件刷新
    std::thread::sleep(std::time::Duration::from_millis(800));

    let files = gather_segments(&session);
    let cfg = state.lock().unwrap().config.clone();
    let merged = session.work_dir.join("merged.flv");
    let ts = now_ts();
    let final_name = build_final_name(&cfg, room_id, ts);
    let final_path = cfg.output_dir.join(&final_name);

    if files.is_empty() {
        let _ = app.emit(
            "recording_stopped",
            serde_json::json!({ "room_id": room_id, "id": "", "error": "无有效分片" }),
        );
        return Err(anyhow!(
            "本次录制没有产生有效的视频文件。\n\
             可能原因：\n\
             ① 录制时间太短（< 1 秒）\n\
             ② 直播流地址无效或主播已下播\n\
             请确认直播间正在直播后再尝试录制。"
        ));
    }

    if files.len() > 1 {
        if let Err(e) = transcode::merge_segments(&files, &merged) {
            let _ = app.emit(
                "recording_stopped",
                serde_json::json!({ "room_id": room_id, "id": "", "error": e.to_string() }),
            );
            let work = session.work_dir.clone();
            return Err(anyhow!(
                "合并失败: {}。原始分片保留在: {}",
                e,
                work.display()
            ));
        }
    } else if let Some(f) = files.first() {
        if let Err(e) = std::fs::copy(f, &merged) {
            let _ = app.emit(
                "recording_stopped",
                serde_json::json!({ "room_id": room_id, "id": "", "error": e.to_string() }),
            );
            return Err(anyhow!("复制分片失败: {}", e));
        }
    }

    if !merged.exists() || merged.metadata().map(|m| m.len()).unwrap_or(0) == 0 {
        // 保留 work_dir 以便恢复
        let work = session.work_dir.clone();
        let _ = app.emit(
            "recording_stopped",
            serde_json::json!({ "room_id": room_id, "id": "", "error": "merged.mp4 is empty" }),
        );
        return Err(anyhow!(
            "合并后无有效数据。原始分片保留在: {}",
            work.display()
        ));
    }

    // 转码：成功才删 work_dir；失败保留供用户恢复
    let effective_fmt = if cfg.auto_mp4 { "mp4" } else { &cfg.output_format };
    if let Err(e) = transcode::transcode(&merged, &final_path, effective_fmt) {
        let work = session.work_dir.clone();
        let _ = app.emit(
            "recording_stopped",
            serde_json::json!({ "room_id": room_id, "id": "", "error": e.to_string() }),
        );
        return Err(anyhow!(
            "转码失败: {}。原始分片保留在: {}",
            e,
            work.display()
        ));
    }

    // 转码成功，清理 work_dir
    let _ = std::fs::remove_dir_all(&session.work_dir);

    // 循环模式：重置状态机，让检测器可以再次触发 Start
    if keep_detector {
        if let Some(logic) = state.lock().unwrap().logic.get_mut(room_id) {
            logic.reset();
        }
    }

    let info = RecordingInfo {
        id: final_name.clone(),
        room_id: room_id.to_string(),
        path: final_path.to_string_lossy().into(),
        size_bytes: std::fs::metadata(&final_path).map(|m| m.len()).unwrap_or(0),
        duration_sec: 0.0,
        format: effective_fmt.to_string(),
        created_at: ts,
    };
    let _ = app.emit("recording_stopped", serde_json::json!(&info));
    Ok(info)
}

/// 列出输出目录下的已完成录制
pub fn list_recordings_in(dir: &Path) -> Vec<RecordingInfo> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            // 跳过 work_dir 临时目录
            if p
                .file_name()
                .map(|n| n.to_string_lossy().starts_with("._work"))
                .unwrap_or(false)
            {
                continue;
            }
            if let Some(ext) = p.extension().and_then(|x| x.to_str()) {
                if VIDEO_EXTS.contains(&ext) {
                    let meta = std::fs::metadata(&p);
                    let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                    let created = meta
                        .and_then(|m| m.modified())
                        .map(|t| {
                            t.duration_since(UNIX_EPOCH)
                                .map(|d| d.as_secs() as i64)
                                .unwrap_or(0)
                        })
                        .unwrap_or(0);
                    let name = p
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let room = name.split('_').next().unwrap_or("").to_string();
                    out.push(RecordingInfo {
                        id: name.clone(),
                        room_id: room,
                        path: p.to_string_lossy().into(),
                        size_bytes: size,
                        duration_sec: 0.0,
                        format: ext.to_string(),
                        created_at: created,
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    out
}