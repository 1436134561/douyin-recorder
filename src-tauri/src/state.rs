use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::config::AppConfig;
use crate::detector::DetectorHandle;
use crate::logic::SitStandLogic;
use crate::monitor::{MonitorHandle, MonitorState};
use crate::recorder::RecordingSession;

/// 全局共享状态（由 Tauri manage 托管）
pub struct AppState {
    pub config: AppConfig,
    /// room_id -> 录制会话
    pub recordings: HashMap<String, RecordingSession>,
    /// room_id -> 检测器句柄
    pub detectors: HashMap<String, DetectorHandle>,
    /// room_id -> 坐立状态机
    pub logic: HashMap<String, SitStandLogic>,
    /// room_id -> monitor 轮询线程句柄
    pub monitors: HashMap<String, MonitorHandle>,
    /// room_id -> monitor 最近一次探测状态（前端通过 monitor_event 实时刷新 + get_status 兜底）
    pub monitor_states: HashMap<String, MonitorState>,
}

impl AppState {
    pub fn new() -> Self {
        AppState {
            config: crate::config::load_config(),
            recordings: HashMap::new(),
            detectors: HashMap::new(),
            logic: HashMap::new(),
            monitors: HashMap::new(),
            monitor_states: HashMap::new(),
        }
    }
}

pub type SharedState = Arc<Mutex<AppState>>;

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
