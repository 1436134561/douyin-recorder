<script setup lang="ts">
import { onMounted } from 'vue'
import { useStore } from '../stores/app'
import Icon from './Icon.vue'

const store = useStore()

const formats = ['mp4', 'mkv', 'mov', 'webm']
const modes = [
  { v: 'auto', label: '自动（有流地址用流，否则屏幕）' },
  { v: 'stream', label: '仅直播流' },
  { v: 'screen', label: '仅屏幕捕获' },
]

onMounted(() => {
  store.fetchAutostart()
})
</script>

<template>
  <div class="p-8 max-w-3xl mx-auto" v-if="store.config">
    <header class="mb-6">
      <h1 class="text-2xl font-semibold text-ink-800">设置</h1>
      <p class="text-sm text-ink-400 mt-1">录制输出、检测灵敏度与系统行为。</p>
    </header>

    <div class="space-y-4">
      <!-- 输出 -->
      <div class="card p-5">
        <h2 class="font-semibold text-ink-800 mb-4">输出与转码</h2>
        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
          <div class="sm:col-span-2">
            <label class="label">输出目录</label>
            <input v-model="store.config.output_dir" class="input" />
          </div>
          <div>
            <label class="label">默认输出格式</label>
            <select
              v-model="store.config.output_format"
              class="input"
              :disabled="store.config.auto_mp4"
            >
              <option v-for="f in formats" :key="f" :value="f">{{ f.toUpperCase() }}</option>
            </select>
            <p class="mt-1 text-xs text-ink-400" v-if="store.config.auto_mp4">
              始终转 MP4 已开启，已忽略此设置
            </p>
          </div>
          <div>
            <label class="label">分片时长（分钟）</label>
            <input
              v-model.number="store.config.segment_minutes"
              type="number"
              min="1"
              class="input"
            />
          </div>
        </div>

        <div class="mt-5 pt-5 border-t border-ink-100 flex items-center justify-between">
          <div>
            <h3 class="font-medium text-ink-800">始终转码为 MP4</h3>
            <p class="text-xs text-ink-400 mt-1">
              开启后，无论选择何种默认格式，最终输出都是 MP4（兼容性最好）。
              关闭则使用上方选择的格式。
            </p>
          </div>
          <button
            class="relative w-11 h-6 rounded-full transition-colors duration-200"
            :class="store.config.auto_mp4 ? 'bg-brand-600' : 'bg-ink-200'"
            @click="store.config.auto_mp4 = !store.config.auto_mp4"
          >
            <span
              class="absolute top-0.5 left-0.5 w-5 h-5 rounded-full bg-white shadow transition-transform duration-200"
              :class="store.config.auto_mp4 ? 'translate-x-5' : ''"
            ></span>
          </button>
        </div>

        <p class="mt-3 text-xs text-ink-400">
          录制按分片保存 flv，结束后自动合并并转出所选格式。
        </p>
      </div>

      <!-- 捕获 -->
      <div class="card p-5">
        <h2 class="font-semibold text-ink-800 mb-4">捕获方式</h2>
        <div>
          <label class="label">捕获模式</label>
          <select v-model="store.config.capture_mode" class="input">
            <option v-for="m in modes" :key="m.v" :value="m.v">{{ m.label }}</option>
          </select>
        </div>
        <div class="mt-4">
          <label class="label">屏幕捕获源（Windows gdigrab）</label>
          <input
            v-model="store.config.screen_source"
            class="input"
            placeholder="desktop 或 title=窗口标题"
          />
        </div>
        <div class="mt-4">
          <label class="label">Python 路径（留空自动探测）</label>
          <input v-model="store.config.python_path" class="input" placeholder="如 C:\Python\python.exe" />
        </div>
      </div>

      <!-- 检测 -->
      <div class="card p-5">
        <div class="flex items-center justify-between mb-4">
          <h2 class="font-semibold text-ink-800">坐立检测</h2>
          <button
            class="relative w-11 h-6 rounded-full transition-colors duration-200"
            :class="store.config.detect_enabled ? 'bg-brand-600' : 'bg-ink-200'"
            @click="store.config.detect_enabled = !store.config.detect_enabled"
          >
            <span
              class="absolute top-0.5 left-0.5 w-5 h-5 rounded-full bg-white shadow transition-transform duration-200"
              :class="store.config.detect_enabled ? 'translate-x-5' : ''"
            ></span>
          </button>
        </div>

        <div :class="store.config.detect_enabled ? '' : 'opacity-40 pointer-events-none'">
          <div class="mb-5">
            <div class="flex justify-between text-sm mb-1.5">
              <span class="font-medium text-ink-600">检测灵敏度</span>
              <span class="text-brand-600 font-medium">{{ store.config.sensitivity.toFixed(2) }}</span>
            </div>
            <input
              v-model.number="store.config.sensitivity"
              type="range"
              min="0.5"
              max="2"
              step="0.05"
              class="w-full"
            />
            <p class="mt-1 text-xs text-ink-400">
              越高越灵敏，能更快捕捉摆手等小幅动作；过低可能漏检。
            </p>
          </div>
          <div>
            <div class="flex justify-between text-sm mb-1.5">
              <span class="font-medium text-ink-600">坐下停录延迟（秒）</span>
              <span class="text-brand-600 font-medium">{{ store.config.sit_stop_seconds }}s</span>
            </div>
            <input
              v-model.number="store.config.sit_stop_seconds"
              type="range"
              min="1"
              max="30"
              step="1"
              class="w-full"
            />
            <p class="mt-1 text-xs text-ink-400">
              主播持续坐下且无动作超过该时长后自动停止录制（防抖）。
            </p>
          </div>
        </div>
      </div>

      <!-- 系统 -->
      <div class="card p-5">
        <div class="flex items-center justify-between">
          <div>
            <h2 class="font-semibold text-ink-800">开机自启</h2>
            <p class="text-xs text-ink-400 mt-1">系统启动时自动在后台运行本程序。</p>
          </div>
          <button
            class="relative w-11 h-6 rounded-full transition-colors duration-200"
            :class="store.config.autostart ? 'bg-brand-600' : 'bg-ink-200'"
            @click="store.setAutostart(!store.config.autostart)"
          >
            <span
              class="absolute top-0.5 left-0.5 w-5 h-5 rounded-full bg-white shadow transition-transform duration-200"
              :class="store.config.autostart ? 'translate-x-5' : ''"
            ></span>
          </button>
        </div>
      </div>

      <div class="flex justify-end">
        <button class="btn-primary" @click="store.saveConfig()">
          <Icon name="check" /> 保存设置
        </button>
      </div>
    </div>
  </div>
</template>
