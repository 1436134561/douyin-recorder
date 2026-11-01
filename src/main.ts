import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import './style.css'
import { installWindowGuard } from './lib/windowGuard'

createApp(App).use(createPinia()).mount('#app')

// 窗口最小化/隐藏再恢复时 WebView2 可能白屏，这个守卫负责自检并救回来
installWindowGuard()
