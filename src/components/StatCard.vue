<script setup lang="ts">
import Icon from './Icon.vue'

defineProps<{
  label: string
  value: string
  hint?: string
  icon: string
}>()
</script>

<template>
  <div class="card p-5 flex items-start justify-between animate-scale-in">
    <div>
      <div class="text-sm text-ink-500">{{ label }}</div>
      <div class="mt-1.5 text-2xl font-semibold text-ink-800 tracking-tight">
        {{ value }}
      </div>
      <div v-if="hint" class="mt-1 text-xs text-ink-400">{{ hint }}</div>
    </div>
    <div
      class="w-11 h-11 rounded-xl bg-brand-50 text-brand-600 flex items-center justify-center"
    >
      <Icon :name="icon" />
    </div>
  </div>
</template>
