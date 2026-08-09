<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from 'vue'
import { convertFileSrc } from '@tauri-apps/api/core'
import { openPath, revealItemInDir } from '@tauri-apps/plugin-opener'
import { useStore } from '../stores/app'
import type { RecordingInfo, Segment } from '../lib/types'
import Icon from './Icon.vue'
import { formatTime } from '../lib/format'

const props = defineProps<{ file: RecordingInfo }>()
const emit = defineEmits<{ (e: 'close'): void }>()
const store = useStore()

const videoEl = ref<HTMLVideoElement | null>(null)
const trackEl = ref<HTMLElement | null>(null)
const duration = ref(0)
const current = ref<Segment>({ start: 0, end: 0 })
const segments = ref<Segment[]>([])
const dragging = ref<null | 'start' | 'end'>(null)
const loadError = ref<string>('')
const isLoading = ref(false)

const fileUrl = computed(() => {
  try {
    return convertFileSrc(props.file.path)
  } catch {
    return props.file.path
  }
})

const startPct = computed(() =>
  duration.value ? (current.value.start / duration.value) * 100 : 0
)
const endPct = computed(() =>
  duration.value ? (current.value.end / duration.value) * 100 : 0
)

function onMeta() {
  if (!videoEl.value) return
  duration.value = videoEl.value.duration || 0
  if (current.value.end === 0 && duration.value > 0) {
    current.value = { start: 0, end: duration.value }
  }
  loadError.value = ''
}

/**
 * HTML5 video error.code 含义：
 *   1 = MEDIA_ERR_ABORTED    —— 用户中止（不常见）
 *   2 = MEDIA_ERR_NETWORK    —— 网络/加载失败（最常见：asset 协议 403、路径不存在）
 *   3 = MEDIA_ERR_DECODE     —— 解码失败（编码不被 WebView2 支持）
 *   4 = MEDIA_ERR_SRC_NOT_SUPPORTED —— 源不被支持（与 3 类似）
 *
 * 之前只显示一条通用文案无法区分根因；现在按 code 给针对性提示。
 */
function onVideoError() {
  isLoading.value = false
  const err = videoEl.value?.error
  const code = err?.code ?? 0
  const reason =
    code === 1
      ? '加载被中止'
      : code === 2
        ? '加载失败（可能是输出目录不在应用可访问范围内，或文件被占用）'
        : code === 3
          ? '解码失败：WebView2 不支持该编码格式'
          : code === 4
            ? '源不被支持：可能是编码格式或文件已损坏'
            : '未知错误'
  loadError.value = `视频加载失败：${reason}。\n错误码：${code || 'N/A'}\n文件路径：${props.file.path}`
}

function onVideoLoadStart() {
  isLoading.value = true
  loadError.value = ''
}

function onVideoCanPlay() {
  isLoading.value = false
}

function reload() {
  if (!videoEl.value) return
  loadError.value = ''
  isLoading.value = true
  videoEl.value.load()
}

/**
 * 用系统默认播放器打开文件 —— asset:// 403 / 编码不支持时的兜底
 * Windows 下 opener 会调 `start "" <path>`，用户可在 VLC / WMP / PotPlayer 中正常播放
 */
async function openInSystemPlayer() {
  try {
    await openPath(props.file.path)
  } catch (e) {
    store.notify('err', `系统播放器打开失败：${e}`)
  }
}

async function showInFolder() {
  try {
    await revealItemInDir(props.file.path)
  } catch (e) {
    store.notify('err', `在文件夹中显示失败：${e}`)
  }
}

function timeFromEvent(e: PointerEvent): number {
  if (!trackEl.value || !duration.value) return 0
  const rect = trackEl.value.getBoundingClientRect()
  const pct = Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width))
  return pct * duration.value
}

function onMove(e: PointerEvent) {
  if (!dragging.value) return
  const t = timeFromEvent(e)
  if (dragging.value === 'start') {
    current.value.start = Math.min(t, current.value.end - 0.1)
    current.value.start = Math.max(0, current.value.start)
  } else {
    current.value.end = Math.max(t, current.value.start + 0.1)
    current.value.end = Math.min(duration.value, current.value.end)
  }
}
function onUp() {
  dragging.value = null
}

function addSegment() {
  if (current.value.end - current.value.start < 0.2) return
  segments.value.push({ ...current.value })
  segments.value.sort((a, b) => a.start - b.start)
}
function removeSegment(i: number) {
  segments.value.splice(i, 1)
}

function playSegment(s: Segment) {
  if (!videoEl.value) return
  videoEl.value.currentTime = s.start
  videoEl.value.play().catch(() => {})
}

async function doExport() {
  if (segments.value.length === 0) return
  const name = window.prompt('导出文件名', `${props.file.room_id || 'clip'}_edited`)
  if (!name) return
  await store.exportSegments(
    props.file.path,
    segments.value.map((s) => ({ start: s.start, end: s.end })),
    name
  )
  emit('close')
}

onMounted(() => {
  window.addEventListener('pointermove', onMove)
  window.addEventListener('pointerup', onUp)
})
onUnmounted(() => {
  window.removeEventListener('pointermove', onMove)
  window.removeEventListener('pointerup', onUp)
})

watch(
  () => props.file.path,
  () => {
    segments.value = []
    current.value = { start: 0, end: 0 }
    duration.value = 0
  }
)
</script>

<template>
  <div
    class="fixed inset-0 z-50 bg-ink-900/40 backdrop-blur-sm flex items-center justify-center p-4 animate-fade-in"
    @click.self="emit('close')"
  >
    <div class="card w-full max-w-4xl max-h-[90vh] overflow-y-auto p-6">
      <!-- 头部 -->
      <div class="flex items-center justify-between mb-4">
        <div>
          <h2 class="text-lg font-semibold text-ink-800">预览剪辑</h2>
          <p class="text-xs text-ink-400">
            房间 {{ file.room_id || '—' }} · 拖动时间轴选择片段，可添加多段合并
          </p>
        </div>
        <button class="btn-ghost !px-2.5" @click="emit('close')">
          <Icon name="close" />
        </button>
      </div>

      <!-- 播放器 -->
      <div class="relative">
        <video
          ref="videoEl"
          :src="fileUrl"
          class="w-full rounded-xl bg-black aspect-video"
          controls
          preload="auto"
          @loadstart="onVideoLoadStart"
          @loadedmetadata="onMeta"
          @canplay="onVideoCanPlay"
          @error="onVideoError"
        ></video>
        <!-- 加载中遮罩 -->
        <div
          v-if="isLoading && !loadError"
          class="absolute inset-0 flex items-center justify-center bg-black/40 text-white text-sm"
        >
          <Icon name="hourglass" /> 加载中...
        </div>
        <!-- 错误提示 -->
        <div
          v-if="loadError"
          class="absolute inset-0 flex flex-col items-center justify-center bg-black/85 text-white text-sm p-6"
        >
          <Icon name="alert" class="text-rose-400 mb-2" />
          <p class="whitespace-pre-line text-center">{{ loadError }}</p>
          <div class="mt-4 flex flex-wrap gap-2 justify-center">
            <button class="btn-soft" @click="reload">
              <Icon name="refresh" /> 重试
            </button>
            <button class="btn-soft" @click="openInSystemPlayer">
              <Icon name="external" /> 在系统播放器中打开
            </button>
            <button class="btn-soft" @click="showInFolder">
              <Icon name="folder" /> 在文件夹中显示
            </button>
          </div>
        </div>
      </div>

      <!-- 时间轴下方兜底：内置播放器打不开时也能一键外部播放 -->
      <div v-if="loadError" class="mt-3 text-xs text-ink-500 text-center">
        如果多次重试仍失败，请点击上方「在系统播放器中打开」用 VLC / PotPlayer 等播放
      </div>

      <!-- 时间轴 -->
      <div class="mt-5">
        <div class="flex items-center justify-between text-xs text-ink-500 mb-2">
          <span>起点 {{ formatTime(current.start) }}</span>
          <span>已选片段 {{ formatTime(current.end - current.start) }}</span>
          <span>终点 {{ formatTime(current.end) }}</span>
        </div>

        <div
          ref="trackEl"
          class="relative h-12 rounded-xl bg-ink-100 cursor-pointer select-none"
        >
          <!-- 已选区间 -->
          <div
            class="absolute top-0 bottom-0 bg-brand-200/70"
            :style="{ left: startPct + '%', right: 100 - endPct + '%' }"
          ></div>
          <!-- 片段刻度 -->
          <div
            v-for="(s, i) in segments"
            :key="i"
            class="absolute top-0 bottom-0 bg-brand-400/40 border-x border-brand-400"
            :style="{
              left: (duration ? (s.start / duration) * 100 : 0) + '%',
              width: (duration ? ((s.end - s.start) / duration) * 100 : 0) + '%',
            }"
            :title="`${formatTime(s.start)} - ${formatTime(s.end)}`"
          ></div>
          <!-- 起点手柄 -->
          <div
            class="absolute top-1/2 -translate-y-1/2 -translate-x-1/2 w-4 h-4 rounded-full bg-brand-600 shadow ring-2 ring-white cursor-ew-resize"
            :style="{ left: startPct + '%' }"
            @pointerdown.prevent="dragging = 'start'"
          ></div>
          <!-- 终点手柄 -->
          <div
            class="absolute top-1/2 -translate-y-1/2 -translate-x-1/2 w-4 h-4 rounded-full bg-brand-600 shadow ring-2 ring-white cursor-ew-resize"
            :style="{ left: endPct + '%' }"
            @pointerdown.prevent="dragging = 'end'"
          ></div>
        </div>

        <div class="mt-3 flex justify-end">
          <button class="btn-soft" @click="addSegment">
            <Icon name="plus" /> 添加此片段
          </button>
        </div>
      </div>

      <!-- 片段列表 -->
      <div v-if="segments.length" class="mt-4">
        <div class="text-sm font-medium text-ink-600 mb-2">已选片段（将按顺序合并）</div>
        <div class="space-y-2">
          <div
            v-for="(s, i) in segments"
            :key="i"
            class="flex items-center justify-between rounded-xl border border-ink-100 px-4 py-2.5"
          >
            <div class="text-sm text-ink-700">
              片段 {{ i + 1 }}：{{ formatTime(s.start) }} → {{ formatTime(s.end) }}
              <span class="text-ink-400">（{{ formatTime(s.end - s.start) }}）</span>
            </div>
            <div class="flex items-center gap-2">
              <button class="btn-ghost !px-2.5" @click="playSegment(s)">
                <Icon name="play" />
              </button>
              <button class="btn-ghost !px-2.5 text-rose-500" @click="removeSegment(i)">
                <Icon name="trash" />
              </button>
            </div>
          </div>
        </div>
      </div>

      <!-- 底部操作 -->
      <div class="mt-6 flex items-center justify-between">
        <span class="text-xs text-ink-400">
          中间未选部分将自动去除
        </span>
        <button class="btn-primary" :disabled="segments.length === 0" @click="doExport">
          <Icon name="merge" /> 导出合并视频
        </button>
      </div>
    </div>
  </div>
</template>
