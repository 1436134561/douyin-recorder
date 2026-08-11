//! Monitor 模块：独立轮询线程，驱动「直播状态探测 → 开播触发录制+姿势检测 → 下播收尾」
//!
//! 设计要点：
//! - 独立 tokio task，与 detector 解耦。Monitor 管「live 边界」（开播/下播），detector 管「姿势边界」（站立/坐下）。
//! - 阻塞重活（begin_recording / stop_and_finalize）走 spawn_blocking，避免拖死 tokio worker。
//! - 每 `cfg.monitor_poll_secs`（默认 30s）探测一次直播状态。
//! - LiveProbe 复用 reqwest client + ttwid cookie，避免每轮重新注册触发风控。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};

use crate::detector;
use crate::recorder;
use crate::state::{AppState, SharedState};
use crate::stream_resolver::{extract_web_rid, LiveProbe};

/// Monitor 线程句柄
pub struct MonitorHandle {
    pub stop: Arc<AtomicBool>,
}

/// Monitor 最近一次探测状态（前端通过 monitor_event 实时刷新 + get_status 兜底）
#[derive(Clone, Debug)]
pub struct MonitorState {
    pub live: bool,
    pub last_poll_ts: i64,
}

/// 发往前端的 monitor_event payload
#[derive(Clone, Debug, Serialize)]
pub struct MonitorEvent {
    pub room_id: String,
    pub live: bool,
    pub last_poll_ts: i64,
    pub last_error: Option<String>,
}

/// 启动 monitor 轮询线程
///
/// 返回 MonitorHandle，调用方存入 state.monitors。
pub fn start<R: Runtime>(
    room_id: String,
    app: AppHandle<R>,
    state: SharedState,
) -> MonitorHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();
    let room_id_clone = room_id.clone();
    let state_clone = state.clone();

    // tokio async task：每 N 秒探测一次；重活用 spawn_blocking 包装
    tauri::async_runtime::spawn(async move {
        let mut probe = LiveProbe::new();
        let mut last_live: Option<bool> = None;
        while !stop_clone.load(Ordering::SeqCst) {
            let poll_ts = chrono::Utc::now().timestamp();
            let (web_rid_res, live_res) = extract_and_probe(&mut probe, &room_id_clone).await;
            let (web_rid, live, err_msg) = match (web_rid_res, live_res) {
                (Ok(wid), Ok(lv)) => (wid, lv, None),
                (Ok(wid), Err(e)) => (wid, false, Some(e.to_string())),
                (Err(e), _) => {
                    eprintln!("[monitor] 解析房间号失败: {}", e);
                    sleep_with_stop(&stop_clone, poll_interval_secs(&state_clone, &room_id_clone)).await;
                    continue;
                }
            };

            // 记录探测状态到全局 state（供 get_status 兜底返回）
            {
                let mut st = state_clone.lock().unwrap();
                st.monitor_states.insert(
                    room_id_clone.clone(),
                    MonitorState {
                        live,
                        last_poll_ts: poll_ts,
                    },
                );
            }

            // emit monitor_event（前端监听刷新 UI）
            let _ = app.emit(
                "monitor_event",
                MonitorEvent {
                    room_id: room_id_clone.clone(),
                    live,
                    last_poll_ts: poll_ts,
                    last_error: err_msg.clone(),
                },
            );

            // 状态机
            if let Err(e) =
                drive_state_machine(&app, &state_clone, &room_id_clone, &web_rid, live).await
            {
                eprintln!("[monitor] drive_state_machine error: {}", e);
            }

            // 调试：live 边界变化时记一行
            if last_live != Some(live) {
                eprintln!(
                    "[monitor] {} live: {:?} -> {}",
                    room_id_clone,
                    last_live,
                    if live { "开播" } else { "下播/未开播" }
                );
                last_live = Some(live);
            }

            sleep_with_stop(
                &stop_clone,
                poll_interval_secs(&state_clone, &room_id_clone),
            )
            .await;
        }
        eprintln!("[monitor] {} 轮询线程退出", room_id_clone);
    });

    MonitorHandle { stop }
}

/// 优雅停止 monitor 线程
pub fn stop(handle: MonitorHandle) {
    handle.stop.store(true, Ordering::SeqCst);
}

/// 提取 web_rid 并探测直播状态
async fn extract_and_probe(
    probe: &mut LiveProbe,
    room_id: &str,
) -> (Result<String, anyhow::Error>, Result<bool, anyhow::Error>) {
    let web_rid = extract_web_rid(room_id);
    match web_rid {
        Ok(wid) => {
            let live = probe.is_live(&wid).await;
            (Ok(wid), live)
        }
        Err(e) => (Err(e), Err(anyhow::anyhow!("web_rid 解析失败"))),
    }
}

/// Monitor 状态机：live 边界触发 detector/recorder 启停
///
/// live==true：
///   若无存活 detector → 解析 URL → begin_recording(spawn_detector=false) → spawn_detector(armed_start=true)
///   若有存活 detector → 继续（姿势循环自己管录/停）
/// live==false：
///   若录制中 → stop_and_finalize(keep_detaker=false)（同步阻塞 → spawn_blocking）
///   若 detector 在 → stop + 从 state.detectors 移除
async fn drive_state_machine<R: Runtime>(
    app: &AppHandle<R>,
    state: &SharedState,
    room_id: &str,
    _web_rid: &str,
    live: bool,
) -> anyhow::Result<()> {
    let app_clone = app.clone();
    let state_clone = state.clone();
    let room_id_owned = room_id.to_string();

    if live {
        // 开播分支
        let (need_spawn_detector, need_begin_recording) = {
            let st = state.lock().unwrap();
            let det_running = st
                .detectors
                .get(room_id)
                .map(|d| !d.stopped.load(Ordering::SeqCst))
                .unwrap_or(false);
            let recording = st.recordings.contains_key(room_id);
            (!det_running, !recording)
        };

        if need_spawn_detector {
            if need_begin_recording {
                // begin_recording 是同步阻塞（启动 ffmpeg + 写初始分片），扔到阻塞线程池
                let app_for_blocking = app_clone.clone();
                let state_for_blocking = state_clone.clone();
                let rid = room_id_owned.clone();
                let begin_res = tokio::task::spawn_blocking(move || {
                    recorder::begin_recording(&rid, false, &app_for_blocking, &state_for_blocking)
                })
                .await;
                if let Err(e) = begin_res {
                    return Err(anyhow::anyhow!(
                        "begin_recording join error: {}",
                        e
                    ));
                }
                if let Err(e) = begin_res.unwrap() {
                    return Err(anyhow::anyhow!("自动开始录制失败: {}", e));
                }
            }

            // spawn_detector 是同步但较快（仅 std::thread::spawn），同步调用即可
            // 但为了让 tokio worker 不阻塞，包进 spawn_blocking 也行；这里同步调用保持简洁
            let spawn_res = tauri::async_runtime::spawn_blocking(move || {
                spawn_detector_inner(&app_clone, &state_clone, room_id_owned.clone())
            })
            .await;
            if let Err(e) = spawn_res {
                return Err(anyhow::anyhow!("spawn_detector join error: {}", e));
            }
            if let Err(e) = spawn_res.unwrap() {
                return Err(anyhow::anyhow!("启动检测器失败: {}", e));
            }
        }
    } else {
        // 下播分支
        let (need_stop_recording, need_stop_detector) = {
            let mut st = state.lock().unwrap();
            // 注意：先收尾录制，再停 detector
            let need_stop_recording = st.recordings.contains_key(room_id);
            let det = st.detectors.remove(room_id);
            let need_stop_detector = det.is_some();
            if let Some(mut h) = det {
                h.stop();
            }
            (need_stop_recording, need_stop_detector)
        };

        if need_stop_recording {
            let app_for_blocking = app_clone.clone();
            let state_for_blocking = state_clone.clone();
            let rid = room_id_owned.clone();
            let stop_res = tauri::async_runtime::spawn_blocking(move || {
                recorder::stop_and_finalize(&rid, false, &app_for_blocking, &state_for_blocking)
            })
            .await;
            match stop_res {
                Ok(Ok(_info)) => {
                    // 正常收尾
                }
                Ok(Err(e)) => {
                    eprintln!("[monitor] 下播时 stop_and_finalize 出错: {}", e);
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("stop_and_finalize join error: {}", e));
                }
            }
        }

        // 清理 logic map 中的状态机
        if need_stop_detector || need_stop_recording {
            state.lock().unwrap().logic.remove(room_id);
        }
    }

    Ok(())
}

/// spawn_detector 内部实现（避免直接调用 detector::spawn_detector 锁住 state 太久的细节）
fn spawn_detector_inner<R: Runtime>(
    app: &AppHandle<R>,
    state: &SharedState,
    room_id: String,
) -> anyhow::Result<()> {
    // 取 cfg + room
    let (cfg, room) = {
        let st = state.lock().unwrap();
        let cfg = st.config.clone();
        let room = st.config.rooms.iter().find(|r| r.id == room_id).cloned();
        (cfg, room)
    };
    let (source, mode) = recorder::resolve_source(&room_id, &cfg, room.as_ref());
    if mode == "stream" && source.is_empty() {
        return Err(anyhow::anyhow!(
            "房间 {} 未配置直播流地址，无法启动监控",
            room_id
        ));
    }
    let h = detector::spawn_detector(
        room_id.clone(),
        source,
        mode,
        cfg.sensitivity,
        cfg.sit_stop_seconds,
        true, // armed_start: monitor 模式下 detector 自动 Start/Stop 录制
        cfg.python_path.clone(),
        app.clone(),
        state.clone(),
    )?;
    state.lock().unwrap().detectors.insert(room_id, h);
    Ok(())
}

/// 在 sleep 中检查 stop flag（比纯 sleep 更快响应停止）
async fn sleep_with_stop(stop: &AtomicBool, secs: u64) {
    // 每秒检查一次 stop，最长 1 秒响应延迟
    for _ in 0..secs {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// 读取当前 monitor_poll_secs（config 可能在 monitor 运行期间被修改）
fn poll_interval_secs(state: &SharedState, _room_id: &str) -> u64 {
    state
        .lock()
        .unwrap()
        .config
        .monitor_poll_secs
        .clamp(20, 120)
}

// ─── 给 AppState 用的 impl（避免 state.rs 与 monitor.rs 循环依赖） ───

/// 在 AppState 上访问 monitors map 的辅助方法
impl AppState {
    pub fn monitor_handle_for(&self, room_id: &str) -> Option<&MonitorHandle> {
        self.monitors.get(room_id)
    }
    pub fn monitor_state_for(&self, room_id: &str) -> Option<&MonitorState> {
        self.monitor_states.get(room_id)
    }
    pub fn monitor_live(&self, room_id: &str) -> bool {
        self.monitor_states
            .get(room_id)
            .map(|s| s.live)
            .unwrap_or(false)
    }
}