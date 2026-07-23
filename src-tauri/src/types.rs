use serde::{Deserialize, Serialize};

/// 检测器每帧输出的事件
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DetectionEvent {
    /// "standing" | "sitting" | "unknown"
    pub state: String,
    /// 帧间动作强度 0~1
    pub motion: f32,
    /// 置信度 0~1
    pub conf: f32,
}

/// 已完成录制文件信息
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RecordingInfo {
    pub id: String,
    pub room_id: String,
    pub path: String,
    pub size_bytes: u64,
    pub duration_sec: f64,
    pub format: String,
    pub created_at: i64,
}

/// 剪辑片段（秒）
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
}

/// 房间实时状态（前端轮询/事件用）
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RoomStatus {
    pub room_id: String,
    pub recording: bool,
    pub monitoring: bool,
    pub last_state: String,
    pub last_motion: f32,
}

/// 「等待中」的录制（转码失败 / 残留的工作目录，可手动恢复或清理）
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PendingRecording {
    /// 工作目录路径（含原始分片）
    pub work_dir: String,
    /// 该目录下有效分片数
    pub segment_count: u32,
    /// 总字节数
    pub total_bytes: u64,
    /// 房间 ID（从工作目录名解析）
    pub room_id: String,
    /// 最早分片修改时间（Unix 秒）
    pub earliest_ts: i64,
}
