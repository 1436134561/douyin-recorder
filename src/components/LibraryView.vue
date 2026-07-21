<script setup lang="ts">
import { ref, computed } from 'vue'
import { useStore } from '../stores/app'
import Icon from './Icon.vue'
import { formatBytes, formatStamp } from '../lib/format'

const store = useStore()
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
</script>

<template>
  <div class="p-8 max-w-5xl mx-auto">
    <header class="mb-6 flex items-end justify-between">
      <div>
        <h1 class="text-2xl font-semibold text-ink-800">已完成录屏</h1>
        <p class="text-sm text-ink-400 mt-1">
          勾选多个可合并为一段；点击「预览剪辑」可裁剪并导出片段。
        </p>
      </div>
      <button
        class="btn-primary"
        :disabled="selectedCount < 2"
        @click="doMerge"
      >
        <Icon name="merge" /> 合并所选 ({{ selectedCount }})
      </button>
    </header>

    <div class="card divide-y divide-ink-100">
      <div
        v-if="store.recordings.length === 0"
        class="p-10 text-center text-ink-400"
      >
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
          <div class="font-medium text-ink-800 truncate">
            房间 {{ r.room_id || '—' }}
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

        <span class="badge bg-ink-100 text-ink-500 uppercase w-14 justify-center">
          {{ r.format }}
        </span>
      </div>
    </div>
  </div>
</template>
