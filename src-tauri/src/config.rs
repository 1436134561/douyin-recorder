use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RoomConfig {
    pub id: String,
    /// 主播/房间昵称（可选，仅展示用）
    pub name: Option<String>,
    /// 直播流地址（flv/hls）。为空时回退屏幕捕获
    pub stream_url: Option<String>,
    /// 是否启用该房间
    pub enabled: bool,
}

impl RoomConfig {
    pub fn new(id: String) -> Self {
        RoomConfig {
            id,
            name: None,
            stream_url: None,
            enabled: true,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AppConfig {
    /// 录制输出目录
    pub output_dir: PathBuf,
    /// 录制完成后转出的最终格式：mp4/mkv/mov/webm
    pub output_format: String,
    /// 是否始终转码为 MP4（无视 output_format 设置）
    #[serde(default = "default_true")]
    pub auto_mp4: bool,
    /// flv 分片时长（分钟）
    pub segment_minutes: u64,
    /// 是否启用坐立检测控制录制
    pub detect_enabled: bool,
    /// 检测灵敏度 0.5~2.0（越高越灵敏）
    pub sensitivity: f32,
    /// 坐下且持续无动作多少秒后停止录制
    pub sit_stop_seconds: u64,
    /// 是否开机自启
    pub autostart: bool,
    /// 捕获模式：auto / stream / screen
    pub capture_mode: String,
    /// 屏幕捕获源（Windows gdigrab：desktop 或 title=窗口名）
    pub screen_source: Option<String>,
    /// 自定义 Python 路径（留空自动探测）
    pub python_path: Option<String>,
    /// 房间列表
    pub rooms: Vec<RoomConfig>,
}

fn default_true() -> bool { true }

impl Default for AppConfig {
    fn default() -> Self {
        let out = dirs::video_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("抖音直播录屏");
        AppConfig {
            output_dir: out,
            output_format: "mp4".into(),
            auto_mp4: true,
            segment_minutes: 10,
            detect_enabled: true,
            sensitivity: 1.0,
            sit_stop_seconds: 5,
            autostart: false,
            capture_mode: "auto".into(),
            screen_source: Some("desktop".into()),
            python_path: None,
            rooms: vec![],
        }
    }
}

fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("douyin-recorder")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

pub fn load_config() -> AppConfig {
    let p = config_path();
    if let Ok(s) = fs::read_to_string(&p) {
        if let Ok(cfg) = serde_json::from_str::<AppConfig>(&s) {
            return cfg;
        }
    }
    let cfg = AppConfig::default();
    let _ = save_config(&cfg);
    cfg
}

pub fn save_config(cfg: &AppConfig) -> anyhow::Result<()> {
    let dir = config_dir();
    fs::create_dir_all(&dir)?;
    fs::create_dir_all(&cfg.output_dir)?;
    let s = serde_json::to_string_pretty(cfg)?;
    fs::write(config_path(), s)?;
    Ok(())
}

/// 输出目录下的有效录制文件扩展名
pub const VIDEO_EXTS: &[&str] = &["mp4", "mkv", "mov", "webm", "flv", "avi"];
