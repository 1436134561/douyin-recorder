import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type {
  AppConfig,
  RoomConfig,
  RecordingInfo,
  DetectionEvent,
  RoomStatus,
  Segment,
  PendingRecording,
} from './types'

export const api = {
  getConfig: () => invoke<AppConfig>('get_config'),
  saveConfig: (cfg: AppConfig) => invoke<void>('save_config', { cfg }),
  listRooms: () => invoke<RoomConfig[]>('list_rooms'),
  importRooms: (text: string) => invoke<RoomConfig[]>('import_rooms', { text }),
  removeRoom: (id: string) => invoke<void>('remove_room', { id }),
  updateRoom: (room: RoomConfig) => invoke<void>('update_room', { room }),
  startRecording: (roomId: string) =>
    invoke<void>('start_recording', { roomId }),
  startMonitor: (roomId: string) => invoke<void>('start_monitor', { roomId }),
  stopRecording: (roomId: string) =>
    invoke<RecordingInfo>('stop_recording', { roomId }),
  stopMonitor: (roomId: string) => invoke<void>('stop_monitor', { roomId }),
  listRecordings: () => invoke<RecordingInfo[]>('list_recordings'),
  transcodeFile: (path: string, format: string) =>
    invoke<RecordingInfo>('transcode_file', { path, format }),
  mergeVideos: (paths: string[], outputName: string) =>
    invoke<RecordingInfo>('merge_videos', { paths, outputName }),
  exportSegments: (path: string, segments: Segment[], outputName: string) =>
    invoke<RecordingInfo>('export_segments', { path, segments, outputName }),
  setAutostart: (enable: boolean) => invoke<void>('set_autostart', { enable }),
  getAutostart: () => invoke<boolean>('get_autostart'),
  showMain: () => invoke<void>('show_main'),
  hideMain: () => invoke<void>('hide_main'),
  getStatus: (roomId: string) =>
    invoke<RoomStatus>('get_status', { roomId }),
  /** 解析抖音直播间 URL 为真实 FLV/HLS 流地址 */
  resolveRoomUrl: (url: string) =>
    invoke<{ success: boolean; flv?: string; hls?: string; error?: string }>('resolve_room_url', { url }),
  /** 删除一个已完成的录制文件 */
  deleteRecording: (path: string) => invoke<void>('delete_recording', { path }),
  /** 列出「等待中」的录制（转码失败残留） */
  listPendingRecordings: () => invoke<PendingRecording[]>('list_pending_recordings'),
  /** 清理「等待中」录制的工作目录 */
  cleanupPendingRecording: (workDir: string) =>
    invoke<void>('cleanup_pending_recording', { workDir }),
  on: (event: string, cb: (payload: unknown) => void): Promise<UnlistenFn> =>
    listen(event, (e) => cb(e.payload)),
}
