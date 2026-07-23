import { defineStore } from 'pinia'
import { reactive, ref } from 'vue'
import { api } from '../lib/api'
import type {
  AppConfig,
  RoomConfig,
  RecordingInfo,
  DetectionEvent,
  RoomStatus,
  Segment,
  PendingRecording,
} from '../lib/types'

export const useStore = defineStore('app', () => {
  const config = ref<AppConfig | null>(null)
  const rooms = ref<RoomConfig[]>([])
  const recordings = ref<RecordingInfo[]>([])
  const pendingRecordings = ref<PendingRecording[]>([])
  const statuses = reactive<Record<string, RoomStatus>>({})
  const detections = reactive<Record<string, DetectionEvent>>({})
  const editorOpen = ref(false)
  const editorFile = ref<RecordingInfo | null>(null)
  const toast = ref<{ type: 'ok' | 'err'; msg: string } | null>(null)
  let toastTimer: number | undefined

  function notify(type: 'ok' | 'err', msg: string) {
    toast.value = { type, msg }
    if (toastTimer) clearTimeout(toastTimer)
    toastTimer = window.setTimeout(() => (toast.value = null), 3200)
  }

  // 统一包装异步调用：成功可给成功提示，失败显式弹出错误（避免「点了没反应」）
  async function guarded(
    label: string,
    fn: () => Promise<unknown>,
    okMsg?: string,
  ) {
    try {
      await fn()
      if (okMsg) notify('ok', okMsg)
    } catch (e) {
      const msg = e instanceof Error ? e.message : typeof e === 'string' ? e : JSON.stringify(e)
      notify('err', `${label}失败：${msg}`)
    }
  }

  async function init() {
    config.value = await api.getConfig()
    rooms.value = await api.listRooms()
    await refreshRecordings()
    await refreshPending()
    await refreshStatuses()
    api.on('recording_started', () => {
      refreshStatuses()
    })
    api.on('recording_stopped', () => {
      refreshRecordings()
      refreshPending()
      refreshStatuses()
      notify('ok', '录制已完成并合并转码')
    })
    api.on('detection_event', (p) => {
      const e = p as DetectionEvent
      detections[e.room_id] = e
    })
  }

  async function refreshRecordings() {
    recordings.value = await api.listRecordings()
  }

  async function refreshPending() {
    try {
      pendingRecordings.value = await api.listPendingRecordings()
    } catch {
      pendingRecordings.value = []
    }
  }

  async function refreshStatuses() {
    await Promise.all(
      rooms.value.map(async (r) => {
        try {
          statuses[r.id] = await api.getStatus(r.id)
        } catch {
          /* ignore */
        }
      })
    )
  }

  async function saveConfig() {
    if (!config.value) return
    await guarded(
      '保存设置',
      async () => {
        await api.saveConfig(config.value!)
      },
      '设置已保存',
    )
  }

  async function importRooms(text: string) {
    await guarded(
      '导入房间',
      async () => {
        rooms.value = await api.importRooms(text)
        await refreshStatuses()
      },
      `已导入 ${rooms.value.length} 个房间`,
    )
  }

  async function removeRoom(id: string) {
    await guarded('删除房间', async () => {
      await api.removeRoom(id)
      rooms.value = rooms.value.filter((r) => r.id !== id)
      delete statuses[id]
      delete detections[id]
    })
  }

  async function updateRoom(room: RoomConfig) {
    await guarded('更新房间', async () => {
      await api.updateRoom(room)
      const i = rooms.value.findIndex((r) => r.id === room.id)
      if (i >= 0) rooms.value[i] = { ...room }
    })
  }

  async function startRecording(id: string) {
    await guarded(
      '开始录制',
      async () => {
        await api.startRecording(id)
        await refreshStatuses()
      },
      `开始录制 ${id}`,
    )
  }

  async function startMonitor(id: string) {
    await guarded(
      '开始监控',
      async () => {
        await api.startMonitor(id)
        await refreshStatuses()
      },
      `开始监控 ${id}`,
    )
  }

  async function stopRecording(id: string) {
    await guarded('停止录制', async () => {
      await api.stopRecording(id)
      await refreshRecordings()
      await refreshPending()
      await refreshStatuses()
    })
  }

  async function stopMonitor(id: string) {
    await guarded('停止监控', async () => {
      await api.stopMonitor(id)
      await refreshStatuses()
    })
  }

  async function transcode(path: string, format: string) {
    await guarded(
      '转码',
      async () => {
        await api.transcodeFile(path, format)
        await refreshRecordings()
      },
      `已转码为 ${format}`,
    )
  }

  async function mergeSelected(paths: string[], name: string) {
    await guarded(
      '合并',
      async () => {
        await api.mergeVideos(paths, name)
        await refreshRecordings()
      },
      '已合并视频',
    )
  }

  async function exportSegments(path: string, segments: Segment[], name: string) {
    await guarded(
      '导出剪辑',
      async () => {
        await api.exportSegments(path, segments, name)
        await refreshRecordings()
      },
      '已导出剪辑片段',
    )
  }

  function openEditor(file: RecordingInfo) {
    editorFile.value = file
    editorOpen.value = true
  }
  function closeEditor() {
    editorOpen.value = false
    editorFile.value = null
  }

  async function setAutostart(enable: boolean) {
    await guarded(
      '设置开机自启',
      async () => {
        await api.setAutostart(enable)
        if (config.value) config.value.autostart = enable
      },
      enable ? '已开启开机自启' : '已关闭开机自启',
    )
  }

  async function fetchAutostart() {
    const enabled = await api.getAutostart()
    if (config.value) config.value.autostart = enabled
  }

  /** 解析抖音直播间 URL 为真实流地址（供前端预览，含主播名/标题） */
  async function resolveRoomUrl(url: string) {
    const result = await guarded(
      '解析直播间',
      () => api.resolveRoomUrl(url),
    )
    return result as Record<string, unknown> | null
  }

  /** 删除已完成录制 */
  async function deleteRecording(path: string) {
    await guarded('删除录像', async () => {
      await api.deleteRecording(path)
      await refreshRecordings()
    })
  }

  /** 清理「等待中」录制的工作目录 */
  async function cleanupPending(workDir: string) {
    await guarded('清理残留', async () => {
      await api.cleanupPendingRecording(workDir)
      await refreshPending()
    })
  }

  return {
    config,
    rooms,
    recordings,
    pendingRecordings,
    statuses,
    detections,
    editorOpen,
    editorFile,
    toast,
    init,
    refreshRecordings,
    refreshPending,
    refreshStatuses,
    saveConfig,
    importRooms,
    removeRoom,
    updateRoom,
    startRecording,
    startMonitor,
    stopRecording,
    stopMonitor,
    transcode,
    mergeSelected,
    exportSegments,
    openEditor,
    closeEditor,
    setAutostart,
    fetchAutostart,
    resolveRoomUrl,
    deleteRecording,
    cleanupPending,
    notify,
  }
})
