use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, Manager, Runtime, State};

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
pub fn save_config(
    cfg: AppConfig,
    app: AppHandle,
    state: State<SharedState>,
) -> Result<(), String> {
    config::save_config(&cfg).map_err(|e| e.to_string())?;
    // 同步更新 asset 协议 scope：用户改了输出目录后立即对新路径放行，
    // 否则旧目录外的预览依然 403。
    if let Err(e) = app.asset_protocol_scope().allow_directory(&cfg.output_dir, true) {
        eprintln!("[warn] allow new output_dir ({:?}) failed: {}", cfg.output_dir, e);
    }
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

/// 开始监控：启动 monitor 轮询线程
///
/// 新流程：
/// 1. 启动 monitor 轮询线程（每 cfg.monitor_poll_secs 秒探测一次开播状态）
/// 2. monitor 线程检测到开播 → 自动 begin_recording + spawn_detector(armed_start=true)
/// 3. monitor 线程检测到下播 → stop_and_finalize 收尾 + 停 detector
///
/// 即使主播当前未开播也会成功（monitor 线程会持续轮询直到开播）。
#[tauri::command]
pub async fn start_monitor(
    room_id: String,
    app: AppHandle,
    state: State<'_, SharedState>,
) -> Result<(), String> {
    let state_inner = state.inner().clone();
    {
        let mut st = state_inner.lock().unwrap();
        if st.monitors.contains_key(&room_id) {
            return Ok(()); // 已在监控
        }
    }

    // 检查房间配置（不强制要求流地址，monitor 会自己解析）
    let room_exists = state_inner
        .lock()
        .unwrap()
        .config
        .rooms
        .iter()
        .any(|r| r.id == room_id);
    if !room_exists {
        return Err(format!("房间 {} 不存在，请先添加", room_id));
    }

    let handle = crate::monitor::start(room_id.clone(), app, state_inner.clone());
    state_inner
        .lock()
        .unwrap()
        .monitors
        .insert(room_id, handle);
    Ok(())
}

/// 监控核心逻辑（供命令与托盘菜单复用）
///
/// 新版：仅启动 monitor 轮询线程，不再直接 spawn_detector。
pub fn start_monitor_inner(
    _app: &AppHandle,
    state: &Arc<Mutex<crate::state::AppState>>,
    room_id: &str,
) -> anyhow::Result<()> {
    if state.lock().unwrap().monitors.contains_key(room_id) {
        return Ok(()); // 已在监控
    }
    // 同步版本：调用方负责 manage AppHandle（实际由 start_monitor async 版本处理 emit 等）
    // 这里保留接口兼容 tray 调用，但 monitor::start 需要 AppHandle<R>；
    // 托盘菜单请直接调用 commands::start_monitor（async 版本）。
    anyhow::bail!("请使用 async start_monitor；此同步入口仅保留兼容")
}

/// 停止录制（async + spawn_blocking：避免阻塞 WebView 主线程）
///
/// 关键：`stop_and_finalize` 是同步阻塞（杀进程 + 等待 + 合并 + 重新编码 H.264），
/// 大文件可能要几分钟。如果用 sync fn，Tauri 会阻塞 WebView 主线程，
/// Windows 显示「未响应」。这里把重活扔到 tokio 阻塞线程池，主线程立即返回。
#[tauri::command]
pub async fn stop_recording(
    room_id: String,
    app: AppHandle,
    state: State<'_, SharedState>,
) -> Result<RecordingInfo, String> {
    let has_session = state.lock().unwrap().recordings.contains_key(&room_id);
    if !has_session {
        // 幂等：实际没在录制 → 只发事件让前端刷新，不动 detector（保留监控）
        let _ = app.emit(
            "recording_stopped",
            serde_json::json!({ "room_id": room_id, "id": "" }),
        );
        return Ok(RecordingInfo {
            id: String::new(),
            room_id: room_id.clone(),
            path: String::new(),
            size_bytes: 0,
            duration_sec: 0.0,
            format: String::new(),
            created_at: 0,
        });
    }
    // 正在录制：清理 detector/logic（快）
    {
        let mut st = state.lock().unwrap();
        if let Some(mut det) = st.detectors.remove(&room_id) {
            det.stop();
        }
        st.logic.remove(&room_id);
    }

    // 把 stop_and_finalize（重活）扔到阻塞线程池，不阻塞 WebView
    let app_clone = app.clone();
    let state_clone: SharedState = state.inner().clone();
    let rid = room_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        recorder::stop_and_finalize(&rid, false, &app_clone, &state_clone)
    })
    .await
    .map_err(|e| format!("join error: {}", e))?;
    result.map_err(|e| e.to_string())
}

/// 停止监控（停 monitor 线程；若正在录制，则一并停止）
#[tauri::command]
pub fn stop_monitor(room_id: String, state: State<SharedState>) -> Result<(), String> {
    let mut st = state.lock().unwrap();
    // 1. 停 monitor 线程
    if let Some(handle) = st.monitors.remove(&room_id) {
        crate::monitor::stop(handle);
    }
    // 2. 清理 monitor_states
    st.monitor_states.remove(&room_id);
    // 3. 停 detector（可能在 monitor 线程外独立启动的）
    if let Some(mut h) = st.detectors.remove(&room_id) {
        h.stop();
    }
    // 4. 清理 logic
    st.logic.remove(&room_id);
    // 5. 录制由前端另行调用 stop_recording 收尾（keep_detector=false 会自动清理）
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

/// 强制显示主窗口（从任何状态恢复：hidden / minimized / 尺寸异常 → 可见 + 获焦 + 重新合成）
///
/// **白屏根因**：窗口最小化或被 `hide()` 之后，WebView2 的 DirectComposition 目标停止出帧；
/// 而原来的实现只有 `unminimize + show + set_focus`，这三步**不产生任何尺寸变化**，
/// WebView2 收不到 `WM_SIZE` 就不会重建合成表面 —— 于是 HWND 显示了、内容却是空白（白屏），
/// 同时输入路由也没恢复，表现就是「所有按钮都点不动」。
/// 修复：恢复时主动制造一次肉眼不可见的尺寸抖动，逼 WebView2 重新 layout + 合成。
pub fn show_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        // 极端情况窗口可能被缩成 0×0（DPI 切换 / 某些最小化路径），先纠正到可用尺寸
        restore_sane_size(&w);
        let _ = w.set_focus();
        nudge_repaint(w);
    }
}

/// 窗口尺寸异常（0 或过小）时纠正回可用尺寸，避免 WebView2 无法分配渲染表面
fn restore_sane_size<R: Runtime>(w: &tauri::WebviewWindow<R>) {
    const MIN_W: u32 = 640;
    const MIN_H: u32 = 480;
    let Ok(sz) = w.outer_size() else { return };
    if sz.width >= MIN_W && sz.height >= MIN_H {
        return;
    }
    let nw = if sz.width < MIN_W { 1200 } else { sz.width };
    let nh = if sz.height < MIN_H { 800 } else { sz.height };
    let _ = w.set_size(tauri::PhysicalSize::new(nw, nh));
}

/// 强制重绘：制造一次 1px 的尺寸抖动（最大化时跳过，否则会退出最大化），
/// 并让页面再抛一次 resize 事件。
///
/// 必须放到后台线程执行：如果调用方在主线程（托盘菜单 / setup），
/// 在主线程 sleep 会阻塞消息泵，两次 `set_size` 会被合并、抖动失效。
fn nudge_repaint<R: Runtime>(w: tauri::WebviewWindow<R>) {
    std::thread::spawn(move || {
        let maximized = w.is_maximized().unwrap_or(false);
        if !maximized {
            if let Ok(sz) = w.outer_size() {
                if sz.width >= 200 && sz.height >= 200 {
                    let _ = w.set_size(tauri::PhysicalSize::new(sz.width, sz.height + 1));
                    std::thread::sleep(std::time::Duration::from_millis(60));
                    let _ = w.set_size(tauri::PhysicalSize::new(sz.width, sz.height));
                }
            }
        }
        // 通知前端「窗口已恢复」，让前端窗口守卫做一次自检（它才是兜底的那一层）
        let _ = w.eval("window.dispatchEvent(new Event('wb:window-restored'))");
    });
}

/// 前端兜底用：任务栏还原等路径不经过 `show_window`，由前端自检发现渲染停摆后主动调用
#[tauri::command]
pub fn force_repaint_window(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        nudge_repaint(w);
    }
}

#[tauri::command]
pub fn show_main(app: AppHandle) {
    show_window(&app);
}

#[tauri::command]
pub fn hide_main(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
}

#[tauri::command]
pub fn get_status(room_id: String, state: State<SharedState>) -> RoomStatus {
    let st = state.lock().unwrap();
    RoomStatus {
        room_id: room_id.clone(),
        recording: st.recordings.contains_key(&room_id),
        monitoring: st.monitors.contains_key(&room_id),
        // 主播当前是否开播（monitor 线程维护；非监控房间返回 false）
        live: st.monitor_live(&room_id),
        last_state: String::new(),
        last_motion: 0.0,
    }
}

/// 每次开始录制时重新解析抖音直播间 URL 为真实 FLV 流地址。
///
/// 重要：抖音 FLV 地址仅有几分钟有效期。即使 stream_url 已经是真实流地址，
/// 也必须强制重新解析，否则过期后 ffmpeg 能启动但收到 0 字节数据。
///
/// Bug C 修复：之前的 `let _ = do_resolve_and_save(...)` 会吞掉解析错误，导致
/// 主播在播但解析被风控/限流/超时时，旧缓存 stream_url 蒙混过关，
/// ffmpeg 拿过期 URL 启动即失败（退出码 -1414549496 = 0xABABABAB 异常终止）。
/// 现在改为：解析失败直接报错给前端，由 resolve() 提供每个策略的具体失败原因。
async fn resolve_room_stream_if_needed(
    room_id: &str,
    state: &State<'_, SharedState>,
) -> Result<(), String> {
    let re_url = format!("https://live.douyin.com/{}", room_id);

    // 修复 Bug C：去掉 `let _ =`，解析失败立即向上抛
    // resolve() 现在自带详细错误信息（各策略失败原因），这里只需直接传播
    if let Err(e) = do_resolve_and_save(&re_url, room_id, state).await {
        return Err(e);
    }

    // 二次校验：解析成功也确保 stream_url 字段被实际写入了
    let has_stream = {
        let st = state.lock().unwrap();
        st.config
            .rooms
            .iter()
            .find(|r| r.id == room_id)
            .and_then(|r| r.stream_url.clone())
            .map(|url| crate::stream_resolver::is_stream_url(&url))
            .unwrap_or(false)
    };
    if !has_stream {
        return Err("未能获取到可用直播流地址，请确认主播正在直播".into());
    }
    Ok(())
}

/// 执行解析并将结果回写到房间配置
///
/// 关键：加 6 秒超时。抖音 Webcast API 经常慢/限流，不超时会导致「开始录制」卡死。
async fn do_resolve_and_save(
    url_to_resolve: &str,
    room_id: &str,
    state: &State<'_, SharedState>,
) -> Result<(), String> {
    // 6 秒超时；超时后端继续工作但不让用户等太久
    let resolve_future = crate::stream_resolver::resolve(url_to_resolve);
    let resolved = match tokio::time::timeout(std::time::Duration::from_secs(6), resolve_future).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            // resolve() 现在自带详细错误（每个策略失败原因），不再二次包装
            return Err(format!("{}", e));
        }
        Err(_) => {
            return Err(format!(
                "解析直播流地址超时（6 秒），房间 {}。\n\
                 网络较慢或抖音风控拦截，请稍后重试。\n\
                 提示：也可在房间设置里手动粘贴 flv/hls 流地址。",
                room_id
            ));
        }
    };
    let mut st = state.lock().unwrap();
    // 明确记录本次选中的画质档位，便于确认「分辨率没有被降级」
    eprintln!(
        "[resolver] 房间 {} 选用画质 = {}（{}）",
        room_id,
        resolved.quality,
        crate::stream_resolver::quality_hint(&resolved.quality)
    );
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

/// 解析抖音直播间 URL 为真实流地址（供前端预览用，含元数据）
#[tauri::command]
pub async fn resolve_room_url(url: String) -> Result<serde_json::Value, String> {
    match crate::stream_resolver::resolve(&url).await {
        Ok(resolved) => {
            let mut obj = serde_json::json!({
                "success": true,
                "flv": resolved.flv,
                "hls": resolved.hls,
                // 实际选中的清晰度档位（如 FULL_HD1 / HD1 / ORIGIN），便于确认没有被降级
                "quality": resolved.quality,
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
    let (cfg, active_dirs): (AppConfig, Vec<PathBuf>) = {
        let st = state.lock().unwrap();
        let dirs = st
            .recordings
            .values()
            .map(|s| s.work_dir.clone())
            .collect::<Vec<_>>();
        (st.config.clone(), dirs)
    };
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
            // 正在录制的会话目录不算「等待中」（否则用户点恢复会把正在写的分片搬走）
            if active_dirs.iter().any(|d| d == &p) {
                continue;
            }
            // 跳过没有产生任何有效分片的目录（刚创建就失败的，无恢复价值）
            let has_video = std::fs::read_dir(&p)
                .map(|subs| {
                    subs.flatten().any(|s| {
                        let sp = s.path();
                        let ext_ok = sp
                            .extension()
                            .map(|x| x == "flv" || x == "mp4")
                            .unwrap_or(false);
                        ext_ok
                            && std::fs::metadata(&sp).map(|m| m.len() > 10_240).unwrap_or(false)
                    })
                })
                .unwrap_or(false);
            if !has_video {
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
            // 从工作目录名解析房间 ID
            // 目录名格式：`._work_<room>@<timestamp>`（新）/ `._work_<room>`（旧残留）
            let tail = name.trim_start_matches("._work_");
            let room_id = tail.split('@').next().unwrap_or(tail).to_string();
            out.push(crate::types::PendingRecording {
                work_dir: p.to_string_lossy().into(),
                segment_count: segments.len() as u32,
                total_bytes: total,
                room_id,
                earliest_ts: if earliest == i64::MAX { 0 } else { earliest },
            });
        }
    }
    // 最近残留排前面
    out.sort_by(|a, b| b.earliest_ts.cmp(&a.earliest_ts));
    out
}

/// 恢复「等待中」的残留录制：合并分片 → 转码为正式录像 → 清理工作目录
///
/// 这是「等待中」标签页以前只能清理、不能救回数据的修复点。
#[tauri::command]
pub async fn resume_pending_recording(
    work_dir: String,
    state: State<'_, SharedState>,
) -> Result<RecordingInfo, String> {
    let cfg = state.lock().unwrap().config.clone();
    let p = PathBuf::from(&work_dir);

    // 安全检查：必须在 output_dir 下，且是本应用的残留工作目录
    let out_dir = std::fs::canonicalize(&cfg.output_dir).unwrap_or_else(|_| cfg.output_dir.clone());
    let target = std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
    if !target.starts_with(&out_dir) {
        return Err("安全限制：只能恢复 output_dir 下的残留".into());
    }
    let dir_name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if !dir_name.starts_with("._work_") {
        return Err("目标不是录制残留工作目录".into());
    }
    if !target.exists() {
        return Err("工作目录不存在（可能已被清理）".into());
    }

    // 文件名用主播名（房间配置里的 name），没有就用房间号
    let room_id = dir_name
        .trim_start_matches("._work_")
        .split('@')
        .next()
        .unwrap_or("")
        .to_string();
    let label = cfg
        .rooms
        .iter()
        .find(|r| r.id == room_id)
        .and_then(|r| r.name.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| room_id.clone());

    // 用最早分片的修改时间做文件名时间戳，更贴近真实录制时间
    let created_ts = crate::recorder::list_work_dir_segments(&target)
        .first()
        .and_then(|f| std::fs::metadata(f).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or_else(|| chrono::Utc::now().timestamp());

    let cfg_clone = cfg.clone();
    let work = target.clone();
    let label_clone = label.clone();
    // 合并 + 转码是重活，扔到阻塞线程池，别卡住 WebView
    let res = tokio::task::spawn_blocking(move || {
        crate::recorder::finalize_work_dir(&work, &label_clone, &cfg_clone, created_ts)
    })
    .await
    .map_err(|e| format!("join error: {}", e))?;

    res.map_err(|e| e.to_string())
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
