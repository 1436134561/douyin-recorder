<script setup lang="ts">
import Icon from './Icon.vue'

const props = defineProps<{ view: string }>()
const emit = defineEmits<{ (e: 'update:view', v: string): void }>()

const items = [
  { key: 'dashboard', label: '仪表盘', icon: 'dashboard' },
  { key: 'rooms', label: '房间管理', icon: 'rooms' },
  { key: 'library', label: '已完成录屏', icon: 'library' },
  { key: 'settings', label: '设置', icon: 'settings' },
]
</script>

<template>
  <aside
    class="w-60 shrink-0 h-full bg-white border-r border-ink-100 flex flex-col animate-fade-in"
  >
    <div class="px-5 py-5 flex items-center gap-3 border-b border-ink-100">
      <div
        class="w-10 h-10 rounded-xl bg-gradient-to-br from-brand-500 to-brand-700 flex items-center justify-center text-white shadow-sm"
      >
        <Icon name="record" />
      </div>
      <div>
        <div class="font-semibold text-ink-800 leading-tight">抖音直播录屏</div>
        <div class="text-xs text-ink-400">Douyin Recorder</div>
      </div>
    </div>

    <nav class="flex-1 px-3 py-4 space-y-1">
      <button
        v-for="it in items"
        :key="it.key"
        class="w-full flex items-center gap-3 px-3 py-2.5 rounded-xl text-sm font-medium transition-all duration-200"
        :class="
          props.view === it.key
            ? 'bg-brand-50 text-brand-700'
            : 'text-ink-500 hover:bg-ink-50 hover:text-ink-700'
        "
        @click="emit('update:view', it.key)"
      >
        <Icon :name="it.icon" />
        <span>{{ it.label }}</span>
      </button>
    </nav>

    <div class="px-4 py-4 border-t border-ink-100 text-xs text-ink-400">
      <div class="flex items-center gap-2">
        <span class="w-2 h-2 rounded-full bg-emerald-400 animate-pulse"></span>
        后台常驻运行中
      </div>
    </div>
  </aside>
</template>
