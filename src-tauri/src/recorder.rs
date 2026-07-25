use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use anyhow::{anyhow, Result};
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
        .stderr(std::process::Stdio::null());
        spawn_hidden(&mut cmd).map_err(|e| anyhow!("ffmpeg 录制启动失败: {}", e))?
    } else {
        let src = cfg.screen_source.clone().unwrap_or_else(|| "desktop".into());
        let out_file = work.join("rec.mp4");
        let mut cmd = Command::new(ffmpeg_executable());
        cmd.args([
            "-y",
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
        .stderr(std::process::Stdio::null());
        spawn_hidden(&mut cmd).map_err(|e| anyhow!("ffmpeg 屏幕录制启动失败: {}", e))?
    };

    // 启动后短暂等待，验证 ffmpeg 是否异常退出（仅非 0 退出码视为失败）
    std::thread::sleep(std::time::Duration::from_millis(600));
    if let Ok(Some(status)) = child.try_wait() {
        if !status.success() {
            return Err(anyhow!(
                "ffmpeg 启动后立即异常退出（exit code {}）。可能原因：\n\
                 ① 流地址无效或主播未在直播\n\
                 ② ffmpeg 找不到或缺少编解码器\n\
                 请检查直播间是否正在直播，或尝试手动填写 flv 流地址。",
                status.code().unwrap_or(-1)
            ));
        }
    }

    Ok(RecordingSession {
        room_id: room_id.to_string(),
        mode: mode.to_string(),
        work_dir: work,
        process: Some(child),
        started_at: now_ts(),
    })
}

/// 收集有效分片文件
fn gather_segments(session: &RecordingSession) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    if session.mode == "stream" {
        if let Ok(entries) = std::fs::read_dir(&session.work_dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().map(|x| x == "flv").unwrap_or(false) {
                    if let Ok(meta) = std::fs::metadata(&p) {
                        // 阈值 10KB：排除 ffmpeg 创建的空壳文件，保留正常录制的分片
                        if meta.len() > 10_240 {
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
                if meta.len() > 10_240 {
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
            "本次录制没有产生有效的视频文件（阈值 > 10KB）。\n\
             可能原因：\n\
             ① 录制时间太短（< 3 秒）\n\
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