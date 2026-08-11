<script setup lang="ts">
import { computed } from 'vue'
import { useStore } from '../stores/app'
import StatCard from './StatCard.vue'
import Icon from './Icon.vue'
import { formatBytes, formatStamp } from '../lib/format'

const store = useStore()

const totalRooms = computed(() => store.rooms.length)
const recordingCount = computed(
  () => Object.values(store.statuses).filter((s) => s.recording).length
)
const completedCount = computed(() => store.recordings.length)
const totalSize = computed(() =>
  formatBytes(store.recordings.reduce((a, r) => a + (r.size_bytes || 0), 0))
)

const liveRooms = computed(() =>
  store.rooms
    .map((r) => ({ room: r, status: store.statuses[r.id], det: store.detections[r.id] }))
    .filter((x) => x.status?.recording || x.status?.monitoring)
)

const stateLabel: Record<string, string> = {
  standing: '站立',
  sitting: '坐下',
  unknown: '未知',
}
const stateClass: Record<string, string> = {
  standing: 'bg-emerald-50 text-emerald-600',
  sitting: 'bg-amber-50 text-amber-600',
  unknown: 'bg-ink-100 text-ink-500',
}
</script>

<template>
  <div class="p-8 max-w-6xl mx-auto">
    <header class="mb-6">
      <h1 class="text-2xl font-semibold text-ink-800">仪表盘</h1>
      <p class="text-sm text-ink-400 mt-1">
        概览录制状态与已完成内容，所有任务在后台静默运行。
      </p>
    </header>

    <section class="grid grid-cols-2 lg:grid-cols-4 gap-4">
      <StatCard label="房间总数" :value="String(totalRooms)" icon="rooms" />
      <StatCard label="录制中" :value="String(recordingCount)" icon="record" hint="实时捕获直播流" />
      <StatCard label="已完成" :value="String(completedCount)" icon="library" />
      <StatCard label="已占用空间" :value="totalSize" icon="folder" />
    </section>

    <section class="mt-6 grid grid-cols-1 lg:grid-cols-2 gap-4">
      <div class="card p-5">
        <div class="flex items-center gap-2 mb-4">
          <Icon name="eye" class="text-brand-600" />
          <h2 class="font-semibold text-ink-800">实时录制 / 监控</h2>
        </div>
        <div v-if="liveRooms.length === 0" class="text-sm text-ink-400 py-8 text-center">
          当前没有正在录制或监控的房间
        </div>
        <TransitionGroup name="list" tag="div" class="space-y-2">
          <div
            v-for="x in liveRooms"
            :key="x.room.id"
            class="flex items-center justify-between rounded-xl border border-ink-100 px-4 py-3"
          >
            <div>
              <div class="font-medium text-ink-800">{{ x.room.id }}</div>
              <div class="text-xs text-ink-400">
                {{
                  x.status?.recording && !x.status?.monitoring ? '录制中'
                  : x.status?.monitoring && x.status?.recording && x.det?.state === 'sitting' ? '监控中（坐下待停）'
                  : x.status?.monitoring && !x.status?.live ? '监控中（等待开播）'
                  : x.status?.monitoring ? '监控中'
                  : '录制中'
                }}
              </div>
            </div>
            <div class="flex items-center gap-2">
              <span
                v-if="x.det"
                class="badge"
                :class="stateClass[x.det.state] || stateClass.unknown"
              >
                {{ stateLabel[x.det.state] || '未知' }}
              </span>
              <span
                v-if="x.status?.recording"
                class="badge bg-rose-50 text-rose-500"
              >
                <span class="w-1.5 h-1.5 rounded-full bg-rose-500 animate-pulse"></span>
                REC
              </span>
            </div>
          </div>
        </TransitionGroup>
      </div>

      <div class="card p-5">
        <div class="flex items-center gap-2 mb-4">
          <Icon name="library" class="text-brand-600" />
          <h2 class="font-semibold text-ink-800">最近完成</h2>
        </div>
        <div v-if="store.recordings.length === 0" class="text-sm text-ink-400 py-8 text-center">
          还没有已完成的录制
        </div>
        <div v-else class="space-y-2 max-h-80 overflow-y-auto pr-1">
          <div
            v-for="r in store.recordings.slice(0, 8)"
            :key="r.id"
            class="flex items-center justify-between rounded-xl border border-ink-100 px-4 py-3 hover:border-brand-200 transition"
          >
            <div class="min-w-0">
              <div class="font-medium text-ink-800 truncate">{{ r.room_id }}</div>
              <div class="text-xs text-ink-400">{{ formatStamp(r.created_at) }}</div>
            </div>
            <span class="badge bg-ink-100 text-ink-500 uppercase">{{ r.format }}</span>
          </div>
        </div>
      </div>
    </section>
  </div>
</template>
