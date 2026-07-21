export interface RoomConfig {
  id: string
  name?: string | null
  stream_url?: string | null
  enabled: boolean
}

export interface AppConfig {
  output_dir: string
  output_format: string
  segment_minutes: number
  detect_enabled: boolean
  sensitivity: number
  sit_stop_seconds: number
  autostart: boolean
  capture_mode: string
  screen_source?: string | null
  python_path?: string | null
  rooms: RoomConfig[]
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
  last_state: string
  last_motion: number
}

export interface Segment {
  start: number
  end: number
}
