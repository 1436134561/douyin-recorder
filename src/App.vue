<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import Sidebar from './components/Sidebar.vue'
import Dashboard from './components/Dashboard.vue'
import RoomManager from './components/RoomManager.vue'
import SettingsPanel from './components/SettingsPanel.vue'
import LibraryView from './components/LibraryView.vue'
import PreviewEditor from './components/PreviewEditor.vue'
import Icon from './components/Icon.vue'
import ConfirmDialog from './components/ConfirmDialog.vue'
import { useStore } from './stores/app'

const store = useStore()
const view = ref('dashboard')

const current = computed(() => {
  switch (view.value) {
    case 'rooms':
      return RoomManager
    case 'library':
      return LibraryView
    case 'settings':
      return SettingsPanel
    default:
      return Dashboard
  }
})

onMounted(() => store.init())
</script>

<template>
  <div class="flex h-screen w-screen overflow-hidden bg-ink-50 text-ink-800 font-sans">
    <Sidebar v-model:view="view" />

    <main class="flex-1 overflow-y-auto">
      <Transition name="fade" mode="out-in">
        <component :is="current" :key="view" />
      </Transition>
    </main>

    <PreviewEditor
      v-if="store.editorOpen && store.editorFile"
      :file="store.editorFile"
      @close="store.closeEditor"
    />

    <!-- 全局确认对话框 -->
    <ConfirmDialog />

    <!-- Toast -->
    <Transition name="fade">
      <div
        v-if="store.toast"
        class="fixed bottom-6 left-1/2 -translate-x-1/2 z-[60] flex items-center gap-2 px-4 py-3 rounded-xl shadow-card text-sm font-medium animate-scale-in"
        :class="
          store.toast.type === 'ok'
            ? 'bg-ink-800 text-white'
            : 'bg-rose-600 text-white'
        "
      >
        <Icon :name="store.toast.type === 'ok' ? 'check' : 'alert'" />
        {{ store.toast.msg }}
      </div>
    </Transition>
  </div>
</template>
