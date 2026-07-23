<script setup lang="ts">
import { ref, computed } from 'vue'
import { useStore } from '../stores/app'
import Icon from './Icon.vue'
import { formatBytes, formatStamp } from '../lib/format'
import type { RecordingInfo } from '../lib/types'

const store = useStore()
type Tab = 'recording' | 'pending' | 'completed'
const activeTab = ref<Tab>('completed')

const selected = ref<Set<string>>(new Set())
const transcodeFmt = ref<Record<string, string>>({})

const formats = ['mp4', 'mkv', 'mov', 'webm']

function toggle(path: string) {
  const s = new Set(selected.value)
  if (s.has(path)) s.delete(path)
  else s.add(path)
  selected.value = s
}

const selectedCount = computed(() => selected.value.size)

// 三态数量
const recordingCount = computed(() =>
  store.rooms.filter((r) => store.statuses[r.id]?.recording).length,
)
const pendingCount = computed(() => store.pendingRecordings.length)
const completedCount = computed(() => store.recordings.length)

async function doMerge() {
  if (selectedCount.value < 2) return
  const name = window.prompt('合并后的文件名', 'merged')
  if (!name) return
  await store.mergeSelected([...selected.value], name)
  selected.value = new Set()
}

async function doTranscode(r: { path: string; id: string }) {
  const fmt = transcodeFmt.value[r.id] || 'mp4'
  await store.transcode(r.path, fmt)
}

function fmtFor(r: { id: string }) {
  return transcodeFmt.value[r.id] || 'mp4'
}
function setFmt(r: { id: string }, v: string) {
  transcodeFmt.value = { ...transcodeFmt.value, [r.id]: v }
}

async function confirmDelete(r: RecordingInfo) {
  const ok = await store.confirm({
    title: '删除录像',
    message: `确定删除「${r.id}」吗？\n该文件将被永久移除。`,
    confirmText: '删除',
    danger: true,
  })
  if (!ok) return
  await store.deleteRecording(r.path)
}

async function confirmCleanup(workDir: string) {
  const ok = await store.confirm({
    title: '清理残留',
    message: '确定清理该残留工作目录吗？\n其中的分片文件将被永久删除。',
    confirmText: '清理',
    danger: true,
  })
  if (!ok) return
  await store.cleanupPending(workDir)
}

function switchTab(t: Tab) {
  activeTab.value = t
}
</script>

<template>
  <div class="p-8 max-w-5xl mx-auto">
    <header class="mb-6 flex items-end justify-between">
      <div>
        <h1 class="text-2xl font-semibold text-ink-800">录屏管理</h1>
        <p class="text-sm text-ink-400 mt-1">
          查看下载中、等待中、已完成的录屏文件，可预览、合并、转码、清理。
        </p>
      </div>
      <button
        v-if="activeTab === 'completed'"
        class="btn-primary"
        :disabled="selectedCount < 2"
        @click="doMerge"
      >
        <Icon name="merge" /> 合并所选 ({{ selectedCount }})
      </button>
    </header>

    <!-- 三态切换 -->
    <div class="flex gap-1 mb-4 border-b border-ink-100">
      <button
        class="px-4 py-2.5 text-sm font-medium border-b-2 transition -mb-px"
        :class="
          activeTab === 'recording'
            ? 'border-brand-600 text-brand-600'
            : 'border-transparent text-ink-500 hover:text-ink-800'
        "
        @click="switchTab('recording')"
      >
        <span class="inline-flex items-center gap-1.5">
          <span class="w-1.5 h-1.5 rounded-full bg-rose-500 animate-pulse"></span>
          下载中
          <span v-if="recordingCount > 0" class="badge bg-rose-50 text-rose-500 ml-1">{{ recordingCount }}</span>
        </span>
      </button>
      <button
        class="px-4 py-2.5 text-sm font-medium border-b-2 transition -mb-px"
        :class="
          activeTab === 'pending'
            ? 'border-brand-600 text-brand-600'
            : 'border-transparent text-ink-500 hover:text-ink-800'
        "
        @click="switchTab('pending')"
      >
        <span class="inline-flex items-center gap-1.5">
          <Icon name="hourglass" />
          等待中
          <span v-if="pendingCount > 0" class="badge bg-amber-50 text-amber-600 ml-1">{{ pendingCount }}</span>
        </span>
      </button>
      <button
        class="px-4 py-2.5 text-sm font-medium border-b-2 transition -mb-px"
        :class="
          activeTab === 'completed'
            ? 'border-brand-600 text-brand-600'
            : 'border-transparent text-ink-500 hover:text-ink-800'
        "
        @click="switchTab('completed')"
      >
        <span class="inline-flex items-center gap-1.5">
          <Icon name="check" />
          已完成
          <span v-if="completedCount > 0" class="badge bg-emerald-50 text-emerald-600 ml-1">{{ completedCount }}</span>
        </span>
      </button>
    </div>

    <!-- 下载中 -->
    <div v-if="activeTab === 'recording'" class="card divide-y divide-ink-100">
      <div v-if="recordingCount === 0" class="p-10 text-center text-ink-400">
        当前没有正在录制的房间
      </div>
      <div
        v-for="r in store.rooms.filter((x) => store.statuses[x.id]?.recording)"
        :key="r.id"
        class="flex items-center gap-4 p-4"
      >
        <div class="w-10 h-10 rounded-xl bg-rose-50 text-rose-500 flex items-center justify-center shrink-0">
          <span class="w-2 h-2 rounded-full bg-rose-500 animate-pulse"></span>
        </div>
        <div class="min-w-0 flex-1">
          <div class="font-medium text-ink-800 truncate">
            {{ r.name || r.id }}
          </div>
          <div class="text-xs text-ink-400">正在录制中...</div>
        </div>
        <button class="btn-soft" @click="store.stopRecording(r.id)">
          <Icon name="stop" /> 停止录制
        </button>
      </div>
    </div>

    <!-- 等待中 -->
    <div v-else-if="activeTab === 'pending'" class="space-y-3">
      <div v-if="pendingCount === 0" class="card p-10 text-center text-ink-400">
        当前没有残留的录制任务<br />
        <span class="text-xs">转码失败或异常中断的分片会出现在这里，可清理或手动恢复</span>
      </div>
      <div
        v-for="p in store.pendingRecordings"
        :key="p.work_dir"
        class="card p-4 flex items-center gap-4"
      >
        <div class="w-10 h-10 rounded-xl bg-amber-50 text-amber-600 flex items-center justify-center shrink-0">
          <Icon name="hourglass" />
        </div>
        <div class="min-w-0 flex-1">
          <div class="font-medium text-ink-800 truncate">
            房间 {{ p.room_id || '未知' }}
          </div>
          <div class="text-xs text-ink-400">
            {{ p.segment_count }} 个分片 · {{ formatBytes(p.total_bytes) }} ·
            <span class="font-mono text-[10px]">{{ p.work_dir }}</span>
          </div>
        </div>
        <button class="btn-soft text-rose-500" @click="confirmCleanup(p.work_dir)">
          <Icon name="trash" /> 清理残留
        </button>
      </div>
    </div>

    <!-- 已完成 -->
    <div v-else class="card divide-y divide-ink-100">
      <div v-if="store.recordings.length === 0" class="p-10 text-center text-ink-400">
        还没有已完成的录制文件
      </div>

      <div
        v-for="r in store.recordings"
        :key="r.id"
        class="flex items-center gap-4 p-4 hover:bg-ink-50/60 transition"
      >
        <input
          type="checkbox"
          class="w-4 h-4 rounded border-ink-300 text-brand-600 accent-brand-600"
          :checked="selected.has(r.path)"
          @change="toggle(r.path)"
        />
        <div class="w-10 h-10 rounded-xl bg-brand-50 text-brand-600 flex items-center justify-center shrink-0">
          <Icon name="play" />
        </div>
        <div class="min-w-0 flex-1">
          <div class="font-medium text-ink-800 truncate" :title="r.id">
            {{ r.id }}
          </div>
          <div class="text-xs text-ink-400">
            {{ formatStamp(r.created_at) }} · {{ formatBytes(r.size_bytes) }}
          </div>
        </div>

        <button class="btn-soft" @click="store.openEditor(r)">
          <Icon name="scissors" /> 预览剪辑
        </button>

        <div class="flex items-center gap-1">
          <select
            :value="fmtFor(r)"
            class="input !w-20 !py-1.5"
            @change="setFmt(r, ($event.target as HTMLSelectElement).value)"
          >
            <option v-for="f in formats" :key="f" :value="f">{{ f }}</option>
          </select>
          <button class="btn-ghost !px-2.5" title="转码" @click="doTranscode(r)">
            <Icon name="check" />
          </button>
        </div>

        <button
          class="btn-ghost !px-2.5 text-rose-500"
          title="删除"
          @click="confirmDelete(r)"
        >
          <Icon name="trash" />
        </button>

        <span class="badge bg-ink-100 text-ink-500 uppercase w-14 justify-center">
          {{ r.format }}
        </span>
      </div>
    </div>
  </div>
</template>