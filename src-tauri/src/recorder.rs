use std::path::{Path, PathBuf};
use std::process::Command;
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
}

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

    let _ = app.emit("recording_started", serde_json::json!({ "room_id": room_id }));
    Ok(())
}

/// 后台看护线程：每 5 秒检查 ffmpeg 是否还活着 + work_dir 文件是否在增长
///
/// 故障检测：
/// - ffmpeg 子进程死亡 → 尝试保存部分分片 + 发 `recording_failed` 事件
/// - work_dir 文件 180 秒无增长（说明流已停止推送数据）→ 同样处理
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
            let (still_recording, work_dir_opt, child_alive) = {
                let mut st = state.lock().unwrap();
                match st.recordings.get_mut(&room_id) {
                    Some(session) => {
                        let alive = match session.process.as_mut() {
                            Some(c) => c.try_wait().ok().map(|s| s.is_none()).unwrap_or(false),
                            None => false,
                        };
                        (true, Some(session.work_dir.clone()), alive)
                    }
                    None => (false, None, false),
                }
            };

            if !still_recording {
                break;
            }

            // 2. ffmpeg 进程已退出？
            if !child_alive {
                finalize_partial_or_cleanup(
                    &room_id,
                    "录制进程已退出（可能：流地址失效、主播下播、网络断开）",
                    &app,
                    &state,
                );
                break;
            }

            // 3. work_dir 文件大小 + 最近一次增长时间
            let total = work_dir_opt
                .as_deref()
                .and_then(|d| std::fs::read_dir(d).ok())
                .map(|entries| {
                    entries
                        .flatten()
                        .filter_map(|e| std::fs::metadata(e.path()).ok().map(|m| m.len()))
                        .sum::<u64>()
                })
                .unwrap_or(0);

            if total > last_total_bytes {
                last_total_bytes = total;
                last_growth = std::time::Instant::now();
            } else if total == 0 {
                // 启动后 NO_DATA_TIMEOUT 秒内没数据就算早期失效
                if last_growth.elapsed() > NO_DATA_TIMEOUT {
                    finalize_partial_or_cleanup(
                        &room_id,
                        "录制启动后 90 秒内未收到任何数据（流地址可能无效）",
                        &app,
                        &state,
                    );
                    break;
                }
            } else if last_growth.elapsed() > STALE_TIMEOUT {
                finalize_partial_or_cleanup(
                    &room_id,
                    "录制文件 180 秒无增长（流可能已中断）；部分数据已尝试保留",
                    &app,
                    &state,
                );
                break;
            }
        }
    });
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

/// 启动 ffmpeg 录制子进程
fn start_ffmpeg(
    room_id: &str,
    cfg: &AppConfig,
    room: Option<&RoomConfig>,
) -> Result<RecordingSession> {
    std::fs::create_dir_all(&cfg.output_dir)?;
    let work = cfg.output_dir.join(format!("._work_{}", sanitize_room_id(room_id)));
    std::fs::create_dir_all(&work)?;

    let mode = decide_mode(cfg, room);
    let mut child = if mode == "stream" {
        let url = room
            .and_then(|r| r.stream_url.clone())
            .or_else(|| resolve_stream_url(room_id))
            .ok_or_else(|| anyhow!("房间 {} 未配置直播流地址", room_id))?;
        let seg_time = cfg.segment_minutes.max(1) * 60;
        let out_tmpl = work.join("seg_%03d.flv");
        let mut cmd = Command::new(ffmpeg_executable());
        cmd.args([
            "-y",
            "-v", "info",  // 详细日志：能看到 HTTP 403/网络错误等真实失败原因
            "-i",
            &url,
            "-c",
            "copy",
            "-f",
            "segment",
            "-segment_time",
            &seg_time.to_string(),
            "-segment_format",
            "flv",
            "-reset_timestamps",
            "1",
            "-vsync",
            "0",
            "-reconnect",
            "1",
            "-reconnect_streamed",
            "1",
            "-reconnect_delay_max",
            "5",
            &out_tmpl.to_string_lossy(),
        ])
        .stderr(std::process::Stdio::piped());
        spawn_hidden(&mut cmd).map_err(|e| anyhow!("ffmpeg 录制启动失败: {}", e))?
    } else {
        let src = cfg.screen_source.clone().unwrap_or_else(|| "desktop".into());
        let out_file = work.join("rec.mp4");
        let mut cmd = Command::new(ffmpeg_executable());
        cmd.args([
            "-y",
            "-v", "info",
            "-f",
            "gdigrab",
            "-framerate",
            "30",
            "-i",
            &src,
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-f",
            "mp4",
            &out_file.to_string_lossy(),
        ])
        .stderr(std::process::Stdio::piped());
        spawn_hidden(&mut cmd).map_err(|e| anyhow!("ffmpeg 屏幕录制启动失败: {}", e))?
    };

    // 关键修复：不用后台线程 + Arc<Mutex>，避免竞态。
    // 直接 take stderr 句柄保留；等 ffmpeg 退出后同步排空（管道已关闭，read_to_end 立即返回）
    let stderr_handle: Option<std::process::ChildStderr> = child.stderr.take();

    // 启动后等待 ffmpeg 连接流（流地址解析/握手可能慢；2 秒比之前 0.6 秒更宽裕）
    std::thread::sleep(std::time::Duration::from_millis(2000));
    if let Ok(Some(status)) = child.try_wait() {
        if !status.success() {
            // 同步排空 stderr（管道已随进程退出而关闭，read_to_end 立即返回所有数据）
            let stderr = drain_stderr(stderr_handle);
            return Err(ffmpeg_exit_error(&stderr, status.code(), 2, mode));
        }
    }

    // 再等 3 秒检查：ffmpeg 进程是活着的，且 seg_000.flv 有没有开始收到数据？
    // 若 3 秒后 seg_000.flv 仍是 0 字节 → 流地址大概率已过期（抖音 URL 时效性 ~几分钟）
    std::thread::sleep(std::time::Duration::from_millis(3000));
    let seg_file = work.join("seg_000.flv");
    let has_data = seg_file
        .metadata()
        .map(|m| m.len() > 0)
        .unwrap_or(false);
    if !has_data {
        if let Ok(Some(status)) = child.try_wait() {
            if !status.success() {
                let stderr = drain_stderr(stderr_handle);
                return Err(ffmpeg_exit_error(&stderr, status.code(), 5, mode));
            }
        }
        let stderr = drain_stderr(stderr_handle);
        return Err(anyhow!(
            "开始录制 {} 秒后仍未收到数据。\n\
             可能原因：\n\
             ① 流地址已过期（抖音 FLV 地址时效性 ~几分钟，下次会自动重新解析）\n\
             ② 直播间未在直播或被封禁\n\
             ③ 网络防火墙 / 代理问题\n\
             \n\
             请确认直播间正在直播，并重新开始录制。\n\
             \nffmpeg stderr 尾部：\n{}",
            5,
            stderr
        ));
    }

    // 一切正常：丢弃 stderr 句柄，让 child 句柄继续持有（后续杀进程会清理）
    drop(stderr_handle);

    Ok(RecordingSession {
        room_id: room_id.to_string(),
        mode: mode.to_string(),
        work_dir: work,
        process: Some(child),
        started_at: now_ts(),
    })
}

/// 同步排空 ffmpeg stderr（管道随子进程退出已关闭，read_to_end 立即返回所有数据）
/// 避免后台线程 + Arc<Mutex> 的竞态：之前 read_stderr 可能在后台线程读完前调用，
/// 导致只能看到 banner 而看不到真正的错误信息。
fn drain_stderr(stderr: Option<std::process::ChildStderr>) -> String {
    use std::io::Read;
    let mut s = match stderr {
        Some(s) => s,
        None => return String::from("（stderr 未捕获）"),
    };
    let mut buf = Vec::with_capacity(2048);
    match (&mut s).take(8 * 1024).read_to_end(&mut buf) {
        Ok(_) => {}
        Err(_) => {
            // 退路：best-effort
            let _ = s.read_to_end(&mut buf);
        }
    }
    if buf.is_empty() {
        return String::from("（stderr 无输出，可能 ffmpeg 被外部进程强制终止，如杀毒软件）");
    }
    String::from_utf8_lossy(&buf)
        .chars()
        .take(1500) // 略放宽，可见更多上下文
        .collect()
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
        || stderr.contains("stderr 未捕获");

    // 分类提示（按优先级：abnormal_code > empty_stderr > 关键词）
    let hint = if abnormal_code && empty_stderr {
        // 🚨 最关键的诊断信号：异常退出码 + 空 stderr
        "🚨 高度疑似被杀毒软件 / Windows Defender 拦截：\n\
         · ffmpeg 启动 banner 输出后立即被杀（stderr 无错误消息说明不是 ffmpeg 自己崩）\n\
         · 异常退出码 0xABxxxxxx 是 Windows 调试堆填充 + 进程被外力 kill 的特征\n\
         · 修复方法：把 ffmpeg.exe 和本程序加入 Windows Defender 排除项（白名单）\n\
         · 或临时关闭实时保护验证（设 → 病毒防护 → 管理设置 → 实时保护 关）\n\
         · 参考：https://support.microsoft.com/zh-cn/windows/添加排除项"
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
fn gather_segments(session: &RecordingSession) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    if session.mode == "stream" {
        if let Ok(entries) = std::fs::read_dir(&session.work_dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().map(|x| x == "flv").unwrap_or(false) {
                    if let Ok(meta) = std::fs::metadata(&p) {
                        if meta.len() > 1_024 {
                            files.push(p);
                        }
                    }
                }
            }
        }
        files.sort();
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