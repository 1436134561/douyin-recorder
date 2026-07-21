<script setup lang="ts">
import { ref } from 'vue'
import { useStore } from '../stores/app'
import type { RoomConfig } from '../lib/types'
import Icon from './Icon.vue'

const store = useStore()

const newId = ref('')
const batchText = ref('')
const showBatch = ref(false)
const editingId = ref<string | null>(null)

const draft = ref<RoomConfig>({ id: '', name: null, stream_url: null, enabled: true })

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

async function addOne() {
  const id = newId.value.trim()
  if (!id) return
  await store.importRooms(id)
  newId.value = ''
}

async function importBatch() {
  if (!batchText.value.trim()) return
  await store.importRooms(batchText.value)
  batchText.value = ''
  showBatch.value = false
}

function beginEdit(r: RoomConfig) {
  editingId.value = r.id
  draft.value = { ...r, name: r.name ?? null, stream_url: r.stream_url ?? null }
}
async function saveEdit() {
  if (!editingId.value) return
  await store.updateRoom(draft.value)
  editingId.value = null
}
</script>

<template>
  <div class="p-8 max-w-5xl mx-auto">
    <header class="mb-6 flex items-end justify-between">
      <div>
        <h1 class="text-2xl font-semibold text-ink-800">房间管理</h1>
        <p class="text-sm text-ink-400 mt-1">
          添加抖音房间号，支持批量导入与逐个开始录制 / 监控。
        </p>
      </div>
      <button class="btn-ghost" @click="showBatch = !showBatch">
        <Icon name="upload" /> 批量导入
      </button>
    </header>

    <!-- 单行添加 + 批量导入 -->
    <div class="card p-5 mb-4">
      <div class="flex gap-2">
        <input
          v-model="newId"
          class="input"
          placeholder="输入单个房间号，如 123456789"
          @keyup.enter="addOne"
        />
        <button class="btn-primary" @click="addOne">
          <Icon name="plus" /> 添加
        </button>
      </div>

      <Transition name="fade">
        <div v-if="showBatch" class="mt-4 pt-4 border-t border-ink-100">
          <label class="label">批量房间号（支持换行 / 逗号 / 空格分隔）</label>
          <textarea
            v-model="batchText"
            rows="5"
            class="input resize-none font-mono"
            placeholder="123456&#10;234567&#10;345678"
          ></textarea>
          <div class="mt-3 flex justify-end">
            <button class="btn-primary" @click="importBatch">
              <Icon name="check" /> 导入列表
            </button>
          </div>
        </div>
      </Transition>
    </div>

    <!-- 房间列表 -->
    <div class="space-y-3">
      <div
        v-for="r in store.rooms"
        :key="r.id"
        class="card p-4 animate-fade-in"
      >
        <div class="flex items-center justify-between gap-3">
          <div class="flex items-center gap-3 min-w-0">
            <div
              class="w-10 h-10 rounded-xl bg-ink-50 text-ink-500 flex items-center justify-center shrink-0"
            >
              <Icon name="rooms" />
            </div>
            <div class="min-w-0">
              <div class="font-semibold text-ink-800 truncate">{{ r.id }}</div>
              <div class="text-xs text-ink-400 truncate">
                {{ r.name || '未命名' }}
                <span v-if="r.stream_url" class="text-emerald-500">· 已配置流地址</span>
              </div>
            </div>
          </div>

          <div class="flex items-center gap-2">
            <span
              v-if="store.detections[r.id]"
              class="badge"
              :class="stateClass[store.detections[r.id].state] || stateClass.unknown"
            >
              {{ stateLabel[store.detections[r.id].state] || '未知' }}
            </span>
            <span
              v-if="store.statuses[r.id]?.recording"
              class="badge bg-rose-50 text-rose-500"
            >
              <span class="w-1.5 h-1.5 rounded-full bg-rose-500 animate-pulse"></span>
              REC
            </span>
            <span
              v-else-if="store.statuses[r.id]?.monitoring"
              class="badge bg-brand-50 text-brand-600"
            >
              <Icon name="eye" /> 监控
            </span>
          </div>
        </div>

        <!-- 操作区 -->
        <div class="mt-4 flex flex-wrap gap-2">
          <template v-if="store.statuses[r.id]?.recording">
            <button class="btn-danger" @click="store.stopRecording(r.id)">
              <Icon name="stop" /> 停止录制
            </button>
          </template>
          <template v-else-if="store.statuses[r.id]?.monitoring">
            <button class="btn-danger" @click="store.stopMonitor(r.id)">
              <Icon name="stop" /> 停止监控
            </button>
          </template>
          <template v-else>
            <button class="btn-primary" @click="store.startMonitor(r.id)">
              <Icon name="eye" /> 开始监控
            </button>
            <button class="btn-ghost" @click="store.startRecording(r.id)">
              <Icon name="play" /> 立即录制
            </button>
          </template>
          <button class="btn-ghost ml-auto" @click="beginEdit(r)">
            <Icon name="edit" /> 编辑
          </button>
          <button class="btn-ghost text-rose-500" @click="store.removeRoom(r.id)">
            <Icon name="trash" />
          </button>
        </div>

        <!-- 编辑面板 -->
        <Transition name="fade">
          <div
            v-if="editingId === r.id"
            class="mt-4 pt-4 border-t border-ink-100 grid grid-cols-1 sm:grid-cols-2 gap-3"
          >
            <div>
              <label class="label">昵称</label>
              <input v-model="draft.name" class="input" placeholder="可选" />
            </div>
            <div>
              <label class="label">直播流地址 (flv/hls)</label>
              <input v-model="draft.stream_url" class="input" placeholder="留空则回退屏幕捕获" />
            </div>
            <div class="sm:col-span-2 flex justify-end gap-2">
              <button class="btn-ghost" @click="editingId = null">取消</button>
              <button class="btn-primary" @click="saveEdit">
                <Icon name="check" /> 保存
              </button>
            </div>
          </div>
        </Transition>
      </div>

      <div
        v-if="store.rooms.length === 0"
        class="card p-10 text-center text-ink-400"
      >
        还没有房间，先添加一个吧。
      </div>
    </div>
  </div>
</template>
