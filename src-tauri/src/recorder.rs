use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use tauri::{AppHandle, Emitter};

use crate::config::{AppConfig, RoomConfig, VIDEO_EXTS};
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

/// 根据配置与房间决定录制模式（stream 需有流地址，否则回退 screen）
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

/// 抖音直播流地址解析（反爬/签名处理）。当前需用户在房间设置中手动粘贴流地址，
/// 后续可在此接入解析器。
fn resolve_stream_url(_room_id: &str) -> Option<String> {
    None
}

/// 开始录制。spawn_detector=true 时按配置自动挂载坐立检测器（用于手动开始+检测停录）
pub fn begin_recording(
    room_id: &str,
    spawn_detector: bool,
    app: &AppHandle,
    state: &Arc<Mutex<AppState>>,
) -> Result<()> {
    let (cfg, room) = {
        let st = state.lock().unwrap();
        let cfg = st.config.clone();
        let room = st.config.rooms.iter().find(|r| r.id == room_id).cloned();
        (cfg, room)
    };
    if state.lock().unwrap().recordings.contains_key(room_id) {
        return Ok(()); // 已在录制
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
    let work = cfg.output_dir.join(format!("._work_{}", room_id));
    std::fs::create_dir_all(&work)?;

    let mode = decide_mode(cfg, room);
    let child = if mode == "stream" {
        let url = room
            .and_then(|r| r.stream_url.clone())
            .or_else(|| resolve_stream_url(room_id))
            .ok_or_else(|| anyhow!("房间 {} 未配置直播流地址", room_id))?;
        let seg_time = cfg.segment_minutes.max(1) * 60;
        let out_tmpl = work.join("seg_%03d.flv");
        Command::new("ffmpeg")
            .args([
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
                &out_tmpl.to_string_lossy(),
            ])
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| anyhow!("ffmpeg 录制启动失败: {}", e))?
    } else {
        let src = cfg.screen_source.clone().unwrap_or_else(|| "desktop".into());
        let out_file = work.join("rec.mp4");
        Command::new("ffmpeg")
            .args([
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
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| anyhow!("ffmpeg 屏幕录制启动失败: {}", e))?
    };

    Ok(RecordingSession {
        room_id: room_id.to_string(),
        mode: mode.to_string(),
        work_dir: work,
        process: Some(child),
        started_at: now_ts(),
    })
}

/// 收集录制产生的分片文件（按名称排序）
fn gather_segments(session: &RecordingSession) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    if session.mode == "stream" {
        if let Ok(entries) = std::fs::read_dir(&session.work_dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().map(|x| x == "flv").unwrap_or(false) {
                    files.push(p);
                }
            }
        }
        files.sort();
    } else {
        let f = session.work_dir.join("rec.mp4");
        if f.exists() {
            files.push(f);
        }
    }
    files
}

/// 停止录制并最终化：合并分片 → 转码为目标格式 → 清理临时目录 → 回传事件
pub fn stop_and_finalize(
    room_id: &str,
    app: &AppHandle,
    state: &Arc<Mutex<AppState>>,
) -> Result<RecordingInfo> {
    // 取出会话与检测器
    let (session, det) = {
        let mut st = state.lock().unwrap();
        let s = st.recordings.remove(room_id);
        let d = st.detectors.remove(room_id);
        st.logic.remove(room_id);
        (s, d)
    };
    if let Some(mut d) = det {
        d.stop();
    }
    let mut session = match session {
        Some(s) => s,
        None => return Err(anyhow!("房间 {} 未在录制", room_id)),
    };

    // 停止 ffmpeg
    if let Some(mut c) = session.process.take() {
        let _ = c.kill();
        let _ = c.wait();
    }
    // 等待文件刷新
    std::thread::sleep(std::time::Duration::from_millis(600));

    let files = gather_segments(&session);
    let cfg = state.lock().unwrap().config.clone();
    let merged = session.work_dir.join("merged.flv");
    let final_name = format!("{}_{}.{}", room_id, now_ts(), cfg.output_format);
    let final_path = cfg.output_dir.join(&final_name);

    if files.len() > 1 {
        transcode::merge_segments(&files, &merged)?;
    } else if let Some(f) = files.first() {
        let _ = std::fs::copy(f, &merged);
    }

    if merged.exists() {
        transcode::transcode(&merged, &final_path, &cfg.output_format)?;
    } else if let Some(f) = files.first() {
        transcode::transcode(f, &final_path, &cfg.output_format)?;
    } else {
        return Err(anyhow!("未找到录制分片，可能录制过早结束"));
    }

    let _ = std::fs::remove_dir_all(&session.work_dir);

    let info = RecordingInfo {
        id: final_name.clone(),
        room_id: room_id.to_string(),
        path: final_path.to_string_lossy().into(),
        size_bytes: std::fs::metadata(&final_path).map(|m| m.len()).unwrap_or(0),
        duration_sec: 0.0,
        format: cfg.output_format.clone(),
        created_at: now_ts(),
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
