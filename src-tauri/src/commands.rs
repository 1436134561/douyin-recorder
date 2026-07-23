use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, State, Window};

use crate::autostart;
use crate::config::{self, AppConfig, RoomConfig};
use crate::recorder;
use crate::state::SharedState;
use crate::transcode;
use crate::types::{RecordingInfo, RoomStatus, Segment};

#[tauri::command]
pub fn get_config(state: State<SharedState>) -> AppConfig {
    state.lock().unwrap().config.clone()
}

#[tauri::command]
pub fn save_config(cfg: AppConfig, state: State<SharedState>) -> Result<(), String> {
    config::save_config(&cfg).map_err(|e| e.to_string())?;
    state.lock().unwrap().config = cfg;
    Ok(())
}

#[tauri::command]
pub fn list_rooms(state: State<SharedState>) -> Vec<RoomConfig> {
    state.lock().unwrap().config.rooms.clone()
}

/// 批量导入房间号（支持换行/逗号/空格分隔的文本）
///
/// 智能识别：若输入是抖音直播间 URL，自动提取干净房间号作为 id，
/// 原始 URL 存入 stream_url（后续录制时自动解析为真实流地址）。
#[tauri::command]
pub fn import_rooms(text: String, state: State<SharedState>) -> Result<Vec<RoomConfig>, String> {
    let raws: Vec<String> = text
        .split(|c| c == '\n' || c == ',' || c == ' ' || c == '\t' || c == '\r')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let mut st = state.lock().unwrap();
    let mut set: HashSet<String> = st.config.rooms.iter().map(|r| r.id.clone()).collect();
    for raw in raws {
        // 判断是否为抖音 URL → 提取干净 ID + 存储 URL 到 stream_url
        let (id, stream_url) =
            if crate::stream_resolver::looks_like_douyin_url(&raw) {
                match crate::stream_resolver::extract_web_rid(&raw) {
                    Ok(clean_id) => (clean_id, Some(raw)),
                    Err(_) => (raw.clone(), None), // 提取失败则原样存储
                }
            } else {
                (raw.clone(), None) // 纯房间号 / 其他格式
            };

        if set.contains(&id) {
            continue;
        }
        set.insert(id.clone());
        st.config.rooms.push(RoomConfig {
            id,
            name: None,
            stream_url,
            enabled: true,
        });
    }
    config::save_config(&st.config).map_err(|e| e.to_string())?;
    Ok(st.config.rooms.clone())
}

#[tauri::command]
pub fn remove_room(id: String, state: State<SharedState>) -> Result<(), String> {
    let mut st = state.lock().unwrap();
    st.config.rooms.retain(|r| r.id != id);
    config::save_config(&st.config).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_room(room: RoomConfig, state: State<SharedState>) -> Result<(), String> {
    let mut st = state.lock().unwrap();
    if let Some(r) = st.config.rooms.iter_mut().find(|r| r.id == room.id) {
        *r = room;
    }
    config::save_config(&st.config).map_err(|e| e.to_string())
}

/// 立即开始录制（若开启检测，则挂载坐立检测用于自动停录）
/// 若房间配置的 stream_url 是抖音直播间网页 URL，会自动解析为真实 FLV 地址后再录制。
#[tauri::command]
pub async fn start_recording(
    room_id: String,
    app: AppHandle,
    state: State<'_, SharedState>,
) -> Result<(), String> {
    // 如果 stream_url 是抖音 URL，先解析为真实流地址
    resolve_room_stream_if_needed(&room_id, &state).await?;
    recorder::begin_recording(&room_id, true, &app, state.inner()).map_err(|e| e.to_string())
}

/// 开始监控：仅启动检测器，主播站立/有动作时自动开始，坐下无动作时自动停止
/// 同样支持自动解析直播间 URL。
#[tauri::command]
pub async fn start_monitor(
    room_id: String,
    app: AppHandle,
    state: State<'_, SharedState>,
) -> Result<(), String> {
    resolve_room_stream_if_needed(&room_id, &state).await?;
    start_monitor_inner(&app, state.inner(), &room_id).map_err(|e| e.to_string())
}

/// 监控核心逻辑（供命令与托盘菜单复用）
pub fn start_monitor_inner(
    app: &AppHandle,
    state: &Arc<Mutex<crate::state::AppState>>,
    room_id: &str,
) -> anyhow::Result<()> {
    if state.lock().unwrap().detectors.contains_key(room_id) {
        return Ok(()); // 已在监控
    }
    let (cfg, room) = {
        let st = state.lock().unwrap();
        let cfg = st.config.clone();
        let room = st.config.rooms.iter().find(|r| r.id == room_id).cloned();
        (cfg, room)
    };
    let (source, mode) = recorder::resolve_source(room_id, &cfg, room.as_ref());
    if mode == "stream" && source.is_empty() {
        return Err(anyhow::anyhow!("房间 {} 未配置直播流地址，无法监控", room_id));
    }
    let h = crate::detector::spawn_detector(
        room_id.to_string(),
        source,
        mode,
        cfg.sensitivity,
        cfg.sit_stop_seconds,
        true,
        cfg.python_path.clone(),
        app.clone(),
        state.clone(),
    )?;
    state.lock().unwrap().detectors.insert(room_id.to_string(), h);
    Ok(())
}

#[tauri::command]
pub fn stop_recording(
    room_id: String,
    app: AppHandle,
    state: State<SharedState>,
) -> Result<RecordingInfo, String> {
    recorder::stop_and_finalize(&room_id, &app, state.inner()).map_err(|e| e.to_string())
}

/// 停止监控（同时停止检测与录制）
#[tauri::command]
pub fn stop_monitor(room_id: String, state: State<SharedState>) -> Result<(), String> {
    let mut st = state.lock().unwrap();
    if let Some(mut h) = st.detectors.remove(&room_id) {
        h.stop();
    }
    if st.recordings.contains_key(&room_id) {
        drop(st);
        // 若正在录制，交给 stop_and_finalize 完成收尾
        // 注意：此处无 app handle，交由前端直接调用 stop_recording
    }
    Ok(())
}

#[tauri::command]
pub fn list_recordings(state: State<SharedState>) -> Vec<RecordingInfo> {
    let cfg = state.lock().unwrap().config.clone();
    recorder::list_recordings_in(&cfg.output_dir)
}

#[tauri::command]
pub fn transcode_file(path: String, format: String) -> Result<RecordingInfo, String> {
    let input = std::path::Path::new(&path);
    let out = input.with_extension(&format);
    transcode::transcode(input, &out, &format).map_err(|e| e.to_string())?;
    Ok(RecordingInfo {
        id: out
            .file_stem()
            .map(|s| s.to_string_lossy().into())
            .unwrap_or_default(),
        room_id: String::new(),
        path: out.to_string_lossy().into(),
        size_bytes: std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0),
        duration_sec: 0.0,
        format,
        created_at: 0,
    })
}

/// 合并多个视频为一个文件
#[tauri::command]
pub fn merge_videos(
    paths: Vec<String>,
    output_name: String,
    state: State<SharedState>,
) -> Result<RecordingInfo, String> {
    let cfg = state.lock().unwrap().config.clone();
    let inputs: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let out = cfg.output_dir.join(format!("{}.{}", output_name, cfg.output_format));
    transcode::merge_videos(&inputs, &out).map_err(|e| e.to_string())?;
    Ok(RecordingInfo {
        id: out
            .file_stem()
            .map(|s| s.to_string_lossy().into())
            .unwrap_or_default(),
        room_id: String::new(),
        path: out.to_string_lossy().into(),
        size_bytes: std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0),
        duration_sec: 0.0,
        format: cfg.output_format,
        created_at: 0,
    })
}

/// 预览剪辑：从源视频截取多段并按顺序合并导出
#[tauri::command]
pub fn export_segments(
    path: String,
    segments: Vec<Segment>,
    output_name: String,
    state: State<SharedState>,
) -> Result<RecordingInfo, String> {
    let cfg = state.lock().unwrap().config.clone();
    let input = std::path::Path::new(&path);
    let segs: Vec<(f64, f64)> = segments.iter().map(|s| (s.start, s.end)).collect();
    let out = cfg.output_dir.join(format!("{}.{}", output_name, cfg.output_format));
    transcode::export_segments(input, &segs, &out).map_err(|e| e.to_string())?;
    Ok(RecordingInfo {
        id: out
            .file_stem()
            .map(|s| s.to_string_lossy().into())
            .unwrap_or_default(),
        room_id: String::new(),
        path: out.to_string_lossy().into(),
        size_bytes: std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0),
        duration_sec: 0.0,
        format: cfg.output_format,
        created_at: 0,
    })
}

#[tauri::command]
pub fn set_autostart(enable: bool, app: AppHandle) -> Result<(), String> {
    autostart::set_autostart(&app, enable).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_autostart(app: AppHandle) -> bool {
    autostart::is_autostart_enabled(&app)
}

#[tauri::command]
pub fn show_main(window: Window) {
    let _ = window.show();
    let _ = window.set_focus();
}

#[tauri::command]
pub fn hide_main(window: Window) {
    let _ = window.hide();
}

#[tauri::command]
pub fn get_status(room_id: String, state: State<SharedState>) -> RoomStatus {
    let st = state.lock().unwrap();
    RoomStatus {
        room_id: room_id.clone(),
        recording: st.recordings.contains_key(&room_id),
        monitoring: st.detectors.contains_key(&room_id),
        last_state: String::new(),
        last_motion: 0.0,
    }
}

/// 若房间的 stream_url（或 room_id 本身）是抖音直播间网页 URL，
/// 自动解析为 FLV 流地址并回写配置。
async fn resolve_room_stream_if_needed(
    room_id: &str,
    state: &State<'_, SharedState>,
) -> Result<(), String> {
    let maybe_url = {
        let st = state.lock().unwrap();
        st.config
            .rooms
            .iter()
            .find(|r| r.id == room_id)
            .and_then(|r| r.stream_url.clone())
    };

    // 已有流地址 → 检查是否需要解析
    if let Some(ref url) = maybe_url {
        // 已经是真实流地址，无需解析
        if crate::stream_resolver::is_stream_url(url) {
            return Ok(());
        }
        // stream_url 是抖音网页 URL → 解析它
        if crate::stream_resolver::looks_like_douyin_url(url) {
            return do_resolve_and_save(url, room_id, state).await;
        }
        return Ok(());
    }

    // stream_url 为空 → 检查 room_id 本身是否是抖音 URL（用户直接粘贴了链接作为 ID）
    if crate::stream_resolver::looks_like_douyin_url(room_id) {
        return do_resolve_and_save(room_id, room_id, state).await;
    }

    Ok(())
}

/// 执行解析并将结果回写到房间配置
async fn do_resolve_and_save(
    url_to_resolve: &str,
    room_id: &str,
    state: &State<'_, SharedState>,
) -> Result<(), String> {
    match crate::stream_resolver::resolve(url_to_resolve).await {
        Ok(resolved) => {
            let mut st = state.lock().unwrap();
            // 如果房间名是空的，且解析拿到了主播昵称 → 填充，并尝试重命名已有文件
            if let Some(room) = st.config.rooms.iter_mut().find(|r| r.id == room_id) {
                room.stream_url = Some(resolved.flv);
                let old_name = room.name.clone();
                if room.name.is_none() || room.name.as_deref() == Some("") {
                    if let Some(ref meta) = resolved.meta {
                        if !meta.nickname.is_empty() {
                            room.name = Some(meta.nickname.clone());
                        }
                    }
                }
                // 名称更新了 → 重命名 output_dir 里已有以房间 ID 开头的录制文件
                if old_name.is_none() && room.name.is_some() {
                    let new_name = room.name.clone().unwrap();
                    let _ = rename_room_recordings(
                        &st.config.output_dir,
                        room_id,
                        &new_name,
                        st.config.auto_mp4,
                        &st.config.output_format,
                    );
                }
            }
            let _ = crate::config::save_config(&st.config);
            Ok(())
        }
        Err(e) => Err(format!(
            "自动解析直播流地址失败：{}。请确认主播正在直播，或手动填写 flv/hls 流地址。",
            e
        )),
    }
}

/// 解析抖音直播间 URL 为真实流地址（供前端预览用，含元数据）
#[tauri::command]
pub async fn resolve_room_url(url: String) -> Result<serde_json::Value, String> {
    match crate::stream_resolver::resolve(&url).await {
        Ok(resolved) => {
            let mut obj = serde_json::json!({
                "success": true,
                "flv": resolved.flv,
                "hls": resolved.hls,
            });
            // 附加元数据（主播名、房间标题）
            if let Some(meta) = resolved.meta {
                obj["nickname"] = serde_json::json!(meta.nickname);
                obj["title"] = serde_json::json!(meta.title);
            }
            Ok(obj)
        }
        Err(e) => Ok(serde_json::json!({
            "success": false,
            "error": e.to_string(),
        })),
    }
}

/// 删除一个已完成的录制文件
#[tauri::command]
pub fn delete_recording(path: String, state: State<SharedState>) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err(format!("文件不存在: {}", path));
    }
    // 安全检查：文件必须在 output_dir 下
    let cfg = state.lock().unwrap().config.clone();
    let out_dir = std::fs::canonicalize(&cfg.output_dir).unwrap_or_else(|_| cfg.output_dir.clone());
    let target = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if !target.starts_with(&out_dir) {
        return Err(format!(
            "安全限制：只能删除 output_dir({}) 下的录制文件",
            out_dir.display()
        ));
    }
    std::fs::remove_file(&target).map_err(|e| format!("删除失败: {}", e))?;
    Ok(())
}

/// 列出「等待中」的录制（转码失败残留的 work_dir，可手动恢复/清理）
#[tauri::command]
pub fn list_pending_recordings(state: State<SharedState>) -> Vec<crate::types::PendingRecording> {
    let cfg = state.lock().unwrap().config.clone();
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&cfg.output_dir) {
        for e in entries.flatten() {
            let p = e.path();
            let name = match p.file_name() {
                Some(n) => n.to_string_lossy().to_string(),
                None => continue,
            };
            if !name.starts_with("._work_") {
                continue;
            }
            // 扫描 work_dir 内的 flv/mp4 分片
            let mut segments = Vec::new();
            let mut total: u64 = 0;
            let mut earliest: i64 = i64::MAX;
            if let Ok(subs) = std::fs::read_dir(&p) {
                for s in subs.flatten() {
                    let sp = s.path();
                    let ext_ok = sp
                        .extension()
                        .map(|x| x == "flv" || x == "mp4")
                        .unwrap_or(false);
                    if !ext_ok {
                        continue;
                    }
                    if let Ok(m) = s.metadata() {
                        if m.len() > 10_240 {
                            segments.push(sp.clone());
                            total += m.len();
                            if let Ok(modified) = m.modified() {
                                if let Ok(d) = modified.duration_since(std::time::UNIX_EPOCH) {
                                    let ts = d.as_secs() as i64;
                                    if ts < earliest {
                                        earliest = ts;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if segments.is_empty() {
                continue;
            }
            // 从工作目录名解析房间 ID（去掉前缀 `._work_`）
            let room_id = name.trim_start_matches("._work_").to_string();
            out.push(crate::types::PendingRecording {
                work_dir: p.to_string_lossy().into(),
                segment_count: segments.len() as u32,
                total_bytes: total,
                room_id,
                earliest_ts: if earliest == i64::MAX { 0 } else { earliest },
            });
        }
    }
    out
}

/// 清理「等待中」录制的工作目录（删除残留分片）
#[tauri::command]
pub fn cleanup_pending_recording(work_dir: String, state: State<SharedState>) -> Result<(), String> {
    let p = std::path::Path::new(&work_dir);
    let cfg = state.lock().unwrap().config.clone();
    let out_dir = std::fs::canonicalize(&cfg.output_dir).unwrap_or_else(|_| cfg.output_dir.clone());
    let target = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if !target.starts_with(&out_dir) {
        return Err("安全限制：只能清理 output_dir 下的残留".into());
    }
    if !target.exists() {
        return Err("工作目录不存在".into());
    }
    std::fs::remove_dir_all(&target).map_err(|e| format!("清理失败: {}", e))?;
    Ok(())
}

/// 当房间名被填充后，重命名 output_dir 里以 room_id 开头的录制文件
/// 旧文件名：`<room_id>_<timestamp>.<ext>` → 新文件名：`<anchor_name>_<timestamp>.<ext>`
fn rename_room_recordings(
    output_dir: &std::path::Path,
    room_id: &str,
    new_name: &str,
    auto_mp4: bool,
    output_format: &str,
) -> Result<(), String> {
    use crate::recorder::sanitize_room_id;
    let safe_room = sanitize_room_id(room_id);
    let safe_name = sanitize_room_id(new_name);
    if safe_room == safe_name || safe_room.is_empty() || safe_name.is_empty() {
        return Ok(());
    }
    let effective_ext = if auto_mp4 { "mp4" } else { output_format };

    let entries = std::fs::read_dir(output_dir).map_err(|e| format!("read_dir: {}", e))?;
    for e in entries.flatten() {
        let p = e.path();
        if !p.is_file() {
            continue;
        }
        let stem = match p.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        // 匹配 "<room_id>_<timestamp>" 形式
        let prefix = format!("{}_", safe_room);
        if !stem.starts_with(&prefix) {
            continue;
        }
        let suffix = &stem[prefix.len()..];
        let new_stem = format!("{}_{}", safe_name, suffix);
        let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
        let final_ext = if ext.is_empty() { effective_ext } else { ext };
        let new_path = p.with_file_name(format!("{}.{}", new_stem, final_ext));
        // 避免覆盖已存在的目标
        if new_path.exists() {
            continue;
        }
        if let Err(e) = std::fs::rename(&p, &new_path) {
            eprintln!("重命名 {:?} -> {:?} 失败: {}", p, new_path, e);
        }
    }
    Ok(())
}
