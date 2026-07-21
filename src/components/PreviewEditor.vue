<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from 'vue'
import { convertFileSrc } from '@tauri-apps/api/core'
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
      <video
        ref="videoEl"
        :src="fileUrl"
        class="w-full rounded-xl bg-black aspect-video"
        controls
        @loadedmetadata="onMeta"
      ></video>

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
