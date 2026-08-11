export interface RoomConfig {
  id: string
  name?: string | null
  stream_url?: string | null
  enabled: boolean
}

export interface AppConfig {
  output_dir: string
  output_format: string
  /** 是否始终转码为 MP4（无视 output_format） */
  auto_mp4?: boolean
  segment_minutes: number
  detect_enabled: boolean
  sensitivity: number
  sit_stop_seconds: number
  autostart: boolean
  capture_mode: string
  screen_source?: string | null
  python_path?: string | null
  rooms: RoomConfig[]
  /** 直播状态探测间隔（秒）。monitor 线程每 N 秒探测一次开播。默认 30，范围 20-120 */
  monitor_poll_secs?: number
}

export interface RecordingInfo {
  id: string
  room_id: string
  path: string
  size_bytes: number
  duration_sec: number
  format: string
  created_at: number
}

export interface DetectionEvent {
  room_id: string
  state: string
  motion: number
  conf: number
  decision: string
}

export interface RoomStatus {
  room_id: string
  recording: boolean
  monitoring: boolean
  /** 主播当前是否开播（由 monitor 线程探测；非监控房间为 false） */
  live: boolean
  last_state: string
  last_motion: number
}

/// monitor 线程 emit 的事件 payload
export interface MonitorEvent {
  room_id: string
  live: boolean
  last_poll_ts: number
  last_error?: string | null
}

export interface Segment {
  start: number
  end: number
}

/// 「等待中」的录制（转码失败 / 残留分片，可手动恢复或清理）
export interface PendingRecording {
  work_dir: string
  segment_count: number
  total_bytes: number
  room_id: string
  earliest_ts: number
}
