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

/** 一个剪辑片段（带稳定 id，便于每段拥有自己的剪辑条） */
interface Clip extends Segment {
  id: number
}

const videoEl = ref<HTMLVideoElement | null>(null)
const overviewEl = ref<HTMLElement | null>(null)
const duration = ref(0)
const currentTime = ref(0)
const clips = ref<Clip[]>([])
const loadError = ref<string>('')
const isLoading = ref(false)

/** 正在拖拽的手柄：哪个片段的起点/终点 */
const dragging = ref<{ id: number; which: 'start' | 'end' } | null>(null)
/** 在总览条上框选中的临时区间 */
const selecting = ref<{ from: number; to: number } | null>(null)

let clipSeq = 1
const trackEls = new Map<number, HTMLElement>()
function setTrack(id: number, el: unknown) {
  if (el) trackEls.set(id, el as HTMLElement)
  else trackEls.delete(id)
}

const fileUrl = computed(() => {
  try {
    return convertFileSrc(props.file.path)
  } catch {
    return props.file.path
  }
})

const playheadPct = computed(() => pct(currentTime.value))
const totalSelected = computed(() =>
  clips.value.reduce((s, c) => s + Math.max(0, c.end - c.start), 0),
)

function pct(sec: number): number {
  return duration.value ? (sec / duration.value) * 100 : 0
}

// ─── 时间格式：既显示又支持手输（10:12 / 1:34:00 / 154） ───
function fmtInput(sec: number): string {
  const s = Math.max(0, Math.floor(sec))
  const pad = (n: number) => String(n).padStart(2, '0')
  const h = Math.floor(s / 3600)
  const m = Math.floor((s % 3600) / 60)
  const ss = s % 60
  return h > 0 ? `${h}:${pad(m)}:${pad(ss)}` : `${m}:${pad(ss)}`
}

function parseTimeInput(v: string): number | null {
  const t = v.trim().replace('：', ':')
  if (!t) return null
  if (/^\d+(\.\d+)?$/.test(t)) return parseFloat(t) // 纯数字按「秒」处理
  const parts = t.split(':').map((x) => x.trim())
  if (parts.length < 2 || parts.length > 3) return null
  if (parts.some((p) => p === '' || !/^\d+(\.\d+)?$/.test(p))) return null
  const n = parts.map(Number)
  if (n.length === 2) return n[0] * 60 + n[1]
  return n[0] * 3600 + n[1] * 60 + n[2]
}

// ─── 播放器 ───
function onMeta() {
  if (!videoEl.value) return
  duration.value = videoEl.value.duration || 0
  loadError.value = ''
}

function onTimeUpdate() {
  if (videoEl.value) currentTime.value = videoEl.value.currentTime
}

/**
 * HTML5 video error.code 含义：
 *   1 = MEDIA_ERR_ABORTED / 2 = MEDIA_ERR_NETWORK
 *   3 = MEDIA_ERR_DECODE / 4 = MEDIA_ERR_SRC_NOT_SUPPORTED
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

function seek(t: number) {
  if (!videoEl.value || !duration.value) return
  const v = Math.min(duration.value, Math.max(0, t))
  videoEl.value.currentTime = v
  currentTime.value = v
}

function playClip(c: Clip) {
  if (!videoEl.value) return
  seek(c.start)
  videoEl.value.play().catch(() => {})
}

// ─── 指针交互 ───
function ratioAt(el: HTMLElement | null, e: PointerEvent): number {
  if (!el) return 0
  const r = el.getBoundingClientRect()
  if (r.width <= 0) return 0
  return Math.min(1, Math.max(0, (e.clientX - r.left) / r.width))
}

function timeAt(el: HTMLElement | null, e: PointerEvent): number {
  return ratioAt(el, e) * duration.value
}

function startDrag(id: number, which: 'start' | 'end') {
  dragging.value = { id, which }
}

function onOverviewDown(e: PointerEvent) {
  if (!duration.value) return
  const t = timeAt(overviewEl.value, e)
  seek(t) // 单击也当作定位播放位置
  selecting.value = { from: t, to: t }
}

function onMove(e: PointerEvent) {
  if (dragging.value) {
    const c = clips.value.find((x) => x.id === dragging.value!.id)
    if (!c) return
    const t = timeAt(trackEls.get(c.id) ?? null, e)
    if (dragging.value.which === 'start') {
      c.start = Math.max(0, Math.min(t, c.end - 0.2))
    } else {
      c.end = Math.min(duration.value, Math.max(t, c.start + 0.2))
    }
    return
  }
  if (selecting.value) {
    selecting.value.to = timeAt(overviewEl.value, e)
  }
}

function onUp() {
  if (dragging.value) {
    dragging.value = null
    sortClips()
  }
  if (selecting.value) {
    const a = Math.min(selecting.value.from, selecting.value.to)
    const b = Math.max(selecting.value.from, selecting.value.to)
    selecting.value = null
    if (b - a >= 0.3) addClipRange(a, b)
  }
}

// ─── 片段增删改 ───
function sortClips() {
  clips.value = clips.value.slice().sort((a, b) => a.start - b.start)
}

function addClipRange(startRaw: number, endRaw: number) {
  if (!duration.value) return
  const start = Math.max(0, Math.min(startRaw, endRaw))
  const end = Math.min(duration.value, Math.max(startRaw, endRaw))
  if (end - start < 0.2) return
  clips.value.push({ id: clipSeq++, start, end })
  sortClips()
}

/** 新增一个片段：默认取播放头位置起 60 秒（不足则到片尾） */
function addSegment() {
  if (!duration.value) return
  const start = Math.min(currentTime.value, Math.max(0, duration.value - 0.5))
  const end = Math.min(duration.value, start + 60)
  addClipRange(start, end)
}

function removeClip(id: number) {
  clips.value = clips.value.filter((c) => c.id !== id)
  trackEls.delete(id)
}

function onTimeChange(c: Clip, which: 'start' | 'end', ev: Event) {
  const el = ev.target as HTMLInputElement
  const v = parseTimeInput(el.value)
  if (v === null) {
    store.notify('err', '时间格式无效，请用「分:秒」或「时:分:秒」或纯秒数')
    el.value = fmtInput(which === 'start' ? c.start : c.end)
    return
  }
  if (which === 'start') {
    c.start = Math.max(0, Math.min(v, c.end - 0.2))
    el.value = fmtInput(c.start)
  } else {
    c.end = Math.min(duration.value, Math.max(v, c.start + 0.2))
    el.value = fmtInput(c.end)
  }
  sortClips()
}

async function doExport() {
  if (clips.value.length === 0) return
  const name = window.prompt('导出文件名', `${props.file.room_id || 'clip'}_剪辑`)
  if (!name) return
  const ordered = clips.value
    .slice()
    .sort((a, b) => a.start - b.start)
    .map((c) => ({ start: +c.start.toFixed(3), end: +c.end.toFixed(3) }))
  await store.exportSegments(props.file.path, ordered, name)
  emit('close')
}

onMounted(() => {
  window.addEventListener('pointermove', onMove)
  window.addEventListener('pointerup', onUp)
})
onUnmounted(() => {
  window.removeEventListener('pointermove', onMove)
  window.removeEventListener('pointerup', onUp)
  trackEls.clear()
})

watch(
  () => props.file.path,
  () => {
    clips.value = []
    duration.value = 0
    currentTime.value = 0
    trackEls.clear()
  },
)
</script>

<template>
  <div
    class="fixed inset-0 z-50 bg-ink-900/40 backdrop-blur-sm flex items-center justify-center p-4 animate-fade-in"
    @click.self="emit('close')"
  >
    <div class="card w-full max-w-5xl max-h-[92vh] overflow-y-auto p-6">
      <!-- 头部 -->
      <div class="flex items-center justify-between mb-4">
        <div>
          <h2 class="text-lg font-semibold text-ink-800">预览剪辑</h2>
          <p class="text-xs text-ink-400">
            房间 {{ file.room_id || '—' }} · 总时长 {{ formatTime(duration) }} ·
            每个片段都有独立剪辑条，可加任意多段后一次导出
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
          @timeupdate="onTimeUpdate"
          @seeked="onTimeUpdate"
          @canplay="onVideoCanPlay"
          @error="onVideoError"
        ></video>
        <div
          v-if="isLoading && !loadError"
          class="absolute inset-0 flex items-center justify-center bg-black/40 text-white text-sm"
        >
          <Icon name="hourglass" /> 加载中...
        </div>
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

      <div v-if="loadError" class="mt-3 text-xs text-ink-500 text-center">
        如果多次重试仍失败，请点击上方「在系统播放器中打开」用 VLC / PotPlayer 等播放
      </div>

      <!-- 总览条 -->
      <div class="mt-5">
        <div class="flex items-center justify-between text-xs text-ink-500 mb-2">
          <span class="inline-flex items-center gap-1.5">
            <Icon name="scissors" class="w-3.5 h-3.5" />
            在下面这条总览条上 <b class="text-ink-700">按住拖动</b> 框选新片段，<b class="text-ink-700">单击</b> 定位播放位置
          </span>
          <span>播放头 {{ formatTime(currentTime) }}</span>
        </div>

        <div
          ref="overviewEl"
          class="relative h-14 rounded-xl bg-ink-100 overflow-hidden cursor-crosshair select-none"
          @pointerdown.prevent="onOverviewDown"
        >
          <!-- 已添加的片段 -->
          <div
            v-for="(c, i) in clips"
            :key="c.id"
            class="absolute top-1.5 bottom-1.5 rounded-md bg-brand-400/35 border border-brand-400/70 flex items-center justify-center overflow-hidden"
            :style="{ left: pct(c.start) + '%', width: pct(c.end - c.start) + '%' }"
            :title="`片段 ${i + 1}：${fmtInput(c.start)} → ${fmtInput(c.end)}`"
          >
            <span v-if="pct(c.end - c.start) > 5" class="text-[10px] text-brand-700 font-medium">
              {{ i + 1 }}
            </span>
          </div>

          <!-- 框选中的临时区间 -->
          <div
            v-if="selecting"
            class="absolute top-0 bottom-0 bg-emerald-400/30 border-x-2 border-emerald-500 pointer-events-none"
            :style="{
              left: pct(Math.min(selecting.from, selecting.to)) + '%',
              width: pct(Math.abs(selecting.to - selecting.from)) + '%',
            }"
          ></div>

          <!-- 播放头 -->
          <div
            class="absolute top-0 bottom-0 w-px bg-rose-500 pointer-events-none"
            :style="{ left: playheadPct + '%' }"
          ></div>
        </div>
      </div>

      <!-- 片段剪辑条（每段一条） -->
      <div class="mt-4 space-y-2.5">
        <div v-if="clips.length === 0" class="text-sm text-ink-400 text-center py-6">
          还没有片段 —— 在上面总览条上拖动框选，或点下面「新增一个片段」
        </div>

        <div v-for="(c, i) in clips" :key="c.id" class="rounded-xl border border-ink-100 p-3">
          <!-- 片段头：序号 + 精确时间 + 操作 -->
          <div class="flex items-center gap-3 mb-2 flex-wrap">
            <span class="badge bg-brand-50 text-brand-600">片段 {{ i + 1 }}</span>
            <label class="text-xs text-ink-500 inline-flex items-center gap-1.5">
              起点
              <input
                class="input !w-[80px] !py-1 !px-2 text-xs font-mono"
                :value="fmtInput(c.start)"
                placeholder="10:12"
                @change="onTimeChange(c, 'start', $event)"
              />
            </label>
            <label class="text-xs text-ink-500 inline-flex items-center gap-1.5">
              终点
              <input
                class="input !w-[80px] !py-1 !px-2 text-xs font-mono"
                :value="fmtInput(c.end)"
                placeholder="40:29"
                @change="onTimeChange(c, 'end', $event)"
              />
            </label>
            <span class="text-xs text-ink-400">时长 {{ formatTime(c.end - c.start) }}</span>
            <span class="ml-auto inline-flex items-center gap-1">
              <button class="btn-ghost !px-2" title="从片段起点播放" @click="playClip(c)">
                <Icon name="play" />
              </button>
              <button
                class="btn-ghost !px-2 text-rose-500"
                title="删除该片段"
                @click="removeClip(c.id)"
              >
                <Icon name="trash" />
              </button>
            </span>
          </div>

          <!-- 该片段专属剪辑条 -->
          <div
            :ref="(el) => setTrack(c.id, el)"
            class="relative h-10 rounded-lg bg-ink-100 select-none"
          >
            <div
              class="absolute inset-y-0 bg-brand-400/45 border-x-2 border-brand-500"
              :style="{ left: pct(c.start) + '%', width: pct(c.end - c.start) + '%' }"
            ></div>
            <div
              class="absolute inset-y-0 w-px bg-rose-500/70 pointer-events-none"
              :style="{ left: playheadPct + '%' }"
            ></div>

            <!-- 起点手柄 -->
            <div
              class="absolute top-0 bottom-0 -translate-x-1/2 w-4 flex items-center justify-center cursor-ew-resize"
              :style="{ left: pct(c.start) + '%' }"
              @pointerdown.prevent="startDrag(c.id, 'start')"
            >
              <span class="w-1.5 h-7 rounded-full bg-brand-600 ring-2 ring-white shadow"></span>
            </div>
            <!-- 终点手柄 -->
            <div
              class="absolute top-0 bottom-0 -translate-x-1/2 w-4 flex items-center justify-center cursor-ew-resize"
              :style="{ left: pct(c.end) + '%' }"
              @pointerdown.prevent="startDrag(c.id, 'end')"
            >
              <span class="w-1.5 h-7 rounded-full bg-brand-600 ring-2 ring-white shadow"></span>
            </div>
          </div>
        </div>
      </div>

      <div class="mt-3 flex items-center justify-between flex-wrap gap-2">
        <button class="btn-soft" :disabled="!duration" @click="addSegment">
          <Icon name="plus" /> 新增一个片段
        </button>
        <span class="text-xs text-ink-400">
          共 {{ clips.length }} 段 · 导出后总时长 {{ formatTime(totalSelected) }} · 未选中的部分会被去掉
        </span>
      </div>

      <!-- 底部操作 -->
      <div class="mt-6 flex items-center justify-between">
        <span class="text-xs text-ink-400">
          片段按起点时间顺序拼接导出（可多段，中间部分自动去除）
        </span>
        <button class="btn-primary" :disabled="clips.length === 0" @click="doExport">
          <Icon name="merge" /> 导出合并视频（{{ clips.length }} 段）
        </button>
      </div>
    </div>
  </div>
</template>
