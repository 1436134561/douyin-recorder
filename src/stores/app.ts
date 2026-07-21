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
} from '../lib/types'

export const useStore = defineStore('app', () => {
  const config = ref<AppConfig | null>(null)
  const rooms = ref<RoomConfig[]>([])
  const recordings = ref<RecordingInfo[]>([])
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

  async function init() {
    config.value = await api.getConfig()
    rooms.value = await api.listRooms()
    await refreshRecordings()
    await refreshStatuses()
    api.on('recording_started', () => {
      refreshStatuses()
    })
    api.on('recording_stopped', () => {
      refreshRecordings()
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
    await api.saveConfig(config.value)
    notify('ok', '设置已保存')
  }

  async function importRooms(text: string) {
    rooms.value = await api.importRooms(text)
    await refreshStatuses()
    notify('ok', `已导入 ${rooms.value.length} 个房间`)
  }

  async function removeRoom(id: string) {
    await api.removeRoom(id)
    rooms.value = rooms.value.filter((r) => r.id !== id)
    delete statuses[id]
    delete detections[id]
  }

  async function updateRoom(room: RoomConfig) {
    await api.updateRoom(room)
    const i = rooms.value.findIndex((r) => r.id === room.id)
    if (i >= 0) rooms.value[i] = { ...room }
  }

  async function startRecording(id: string) {
    await api.startRecording(id)
    notify('ok', `开始录制 ${id}`)
    await refreshStatuses()
  }

  async function startMonitor(id: string) {
    await api.startMonitor(id)
    notify('ok', `开始监控 ${id}`)
    await refreshStatuses()
  }

  async function stopRecording(id: string) {
    await api.stopRecording(id)
    await refreshStatuses()
  }

  async function stopMonitor(id: string) {
    await api.stopMonitor(id)
    await refreshStatuses()
  }

  async function transcode(path: string, format: string) {
    await api.transcodeFile(path, format)
    await refreshRecordings()
    notify('ok', `已转码为 ${format}`)
  }

  async function mergeSelected(paths: string[], name: string) {
    await api.mergeVideos(paths, name)
    await refreshRecordings()
    notify('ok', '已合并视频')
  }

  async function exportSegments(path: string, segments: Segment[], name: string) {
    await api.exportSegments(path, segments, name)
    await refreshRecordings()
    notify('ok', '已导出剪辑片段')
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
    await api.setAutostart(enable)
    if (config.value) config.value.autostart = enable
    notify('ok', enable ? '已开启开机自启' : '已关闭开机自启')
  }

  async function fetchAutostart() {
    const enabled = await api.getAutostart()
    if (config.value) config.value.autostart = enabled
  }

  return {
    config,
    rooms,
    recordings,
    statuses,
    detections,
    editorOpen,
    editorFile,
    toast,
    init,
    refreshRecordings,
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
    notify,
  }
})
