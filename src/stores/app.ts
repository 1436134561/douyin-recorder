import { defineStore } from 'pinia'
import { reactive, ref } from 'vue'
import { api } from '../lib/api'
import type {
  AppConfig,
  RoomConfig,
  RecordingInfo,
  DetectionEvent,
  MonitorEvent,
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
  // 状态轻量轮询：覆盖 detector 死亡、stop_and_finalize 同步阻塞期间前端 statuses 陈旧等场景
  let statusPollTimer: number | undefined

  // 全局确认对话框状态（替代浏览器原生 confirm）
  const confirmDialog = ref<{
    open: boolean
    title: string
    message: string
    confirmText: string
    cancelText: string
    danger: boolean
    resolve: ((ok: boolean) => void) | null
  }>({
    open: false,
    title: '',
    message: '',
    confirmText: '确定',
    cancelText: '取消',
    danger: false,
    resolve: null,
  })

  function confirm(opts: {
    title: string
    message: string
    confirmText?: string
    cancelText?: string
    danger?: boolean
  }): Promise<boolean> {
    return new Promise((resolve) => {
      confirmDialog.value = {
        open: true,
        title: opts.title,
        message: opts.message,
        confirmText: opts.confirmText ?? '确定',
        cancelText: opts.cancelText ?? '取消',
        danger: opts.danger ?? false,
        resolve,
      }
    })
  }

  function closeConfirm(ok: boolean) {
    const r = confirmDialog.value.resolve
    confirmDialog.value.open = false
    confirmDialog.value.resolve = null
    if (r) r(ok)
  }

  function notify(type: 'ok' | 'err', msg: string) {
    toast.value = { type, msg }
    if (toastTimer) clearTimeout(toastTimer)
    // 失败 toast 停留 6 秒（更醒目），成功 3.2 秒
    const ms = type === 'err' ? 6000 : 3200
    toastTimer = window.setTimeout(() => (toast.value = null), ms)
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
    api.on('recording_failed', (p) => {
      const e = p as { room_id: string; reason: string }
      refreshStatuses()
      refreshRecordings()
      refreshPending()
      notify('err', `录制已自动停止（${e.room_id}）：${e.reason}`)
    })
    api.on('detection_event', (p) => {
      const e = p as DetectionEvent
      detections[e.room_id] = e
      // 关键：检测器发出 Stop 决策时立刻拉一次最新 statuses。
      // 后端在 emit 该事件后才调 stop_and_finalize（detector.rs:191 → :211），
      // 但 recordings.remove()（recorder.rs:473）在函数最开头，IPC 到达时 recording 已为 false → 无竞态。
      // 不挂 Start 决策是因为 getStatus 拿到的是后端实时状态，Start 后立刻就有。
      if (e.decision === 'Stop') {
        refreshStatuses()
      }
    })
    api.on('monitor_event', (p) => {
      // 后端 monitor 线程每 N 秒探测直播状态后 emit；payload 含 live / last_poll_ts / last_error
      const e = p as MonitorEvent
      // 直接更新 statuses 缓存中的 live 字段，避免等下一次 refreshStatuses
      if (statuses[e.room_id]) {
        statuses[e.room_id].live = e.live
      }
      // 探测失败但监控中 → 提示一下
      if (e.last_error && statuses[e.room_id]?.monitoring) {
        // 不刷屏：只在 live=false 且有错误时静默记录
        // console.warn(`[monitor] ${e.room_id}: ${e.last_error}`)
      }
    })

    // 兜底轮询：3s 一次，覆盖检测器进程死亡、stop_and_finalize 同步阻塞等所有陈旧场景
    if (statusPollTimer === undefined) {
      statusPollTimer = window.setInterval(() => {
        refreshStatuses()
      }, 3000)
    }
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
      const info = await api.stopRecording(id)
      await refreshRecordings()
      await refreshPending()
      await refreshStatuses()
      // 后端幂等：若之前不录制中只刷新了状态，不显示成功提示
      if (info && info.id) {
        notify('ok', `已停止：${info.id}`)
      }
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
  async function resolveRoomUrl(url: string): Promise<Record<string, unknown> | null> {
    try {
      return (await api.resolveRoomUrl(url)) as Record<string, unknown>
    } catch (e) {
      const msg = e instanceof Error ? e.message : typeof e === 'string' ? e : JSON.stringify(e)
      notify('err', `解析直播间失败：${msg}`)
      return null
    }
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
    confirmDialog,
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
    confirm,
    closeConfirm,
    notify,
  }
})
