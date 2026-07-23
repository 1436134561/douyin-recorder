<script setup lang="ts">
import { useStore } from '../stores/app'

const store = useStore()

function onConfirm() {
  store.closeConfirm(true)
}
function onCancel() {
  store.closeConfirm(false)
}
</script>

<template>
  <Teleport to="body">
    <Transition name="fade">
      <div
        v-if="store.confirmDialog.open"
        class="fixed inset-0 bg-ink-900/40 backdrop-blur-sm flex items-center justify-center z-[9999]"
        @click.self="onCancel"
      >
        <div
          class="bg-white rounded-2xl shadow-2xl p-6 max-w-sm w-full mx-4 transform transition-all"
        >
          <h3 class="text-base font-semibold text-ink-800 mb-2">
            {{ store.confirmDialog.title }}
          </h3>
          <p class="text-sm text-ink-500 mb-6 leading-relaxed">
            {{ store.confirmDialog.message }}
          </p>
          <div class="flex justify-end gap-2">
            <button class="btn-soft" @click="onCancel">
              {{ store.confirmDialog.cancelText }}
            </button>
            <button
              class="btn-primary"
              :class="store.confirmDialog.danger ? '!bg-rose-500 hover:!bg-rose-600' : ''"
              @click="onConfirm"
            >
              {{ store.confirmDialog.confirmText }}
            </button>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.18s ease;
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
</style>