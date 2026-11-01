import { api } from './api'

/**
 * 窗口守卫 —— 修复「最小化到任务栏 / 隐藏后再打开 → 白屏且按钮全部点不动」
 *
 * 原理链条：
 * 1. 窗口最小化或被 hide 后，WebView2（Chromium）的合成器停止出帧；
 * 2. 恢复时如果窗口**尺寸没有变化**，WebView2 收不到 WM_SIZE，不会重建
 *    DirectComposition 合成表面 → HWND 显示了但内容是空白（白屏）；
 * 3. 同时 web 内容的输入路由也没跟着恢复 → 鼠标点不到任何按钮。
 *
 * 检测手段：requestAnimationFrame 心跳。
 * 页面处于「可见」状态时如果 1.2 秒内一帧都没跑，说明渲染管线没恢复。
 * 处置分两级：① 请后端做一次强制重绘（1px 尺寸抖动）→ ② 仍不行就 reload 页面兜底。
 * reload 做了 10 秒防抖，不会陷入刷新死循环。
 */
export function installWindowGuard() {
  let lastFrame = performance.now()
  let lastCheckAt = 0
  let lastReloadAt = 0
  let timer = 0

  const tick = () => {
    lastFrame = performance.now()
    requestAnimationFrame(tick)
  }
  requestAnimationFrame(tick)

  async function check(reason: string) {
    // 节流：3 秒内的重复触发忽略（focus / visibilitychange 经常一起到）
    const now = Date.now()
    if (now - lastCheckAt < 3000) return
    lastCheckAt = now

    window.clearTimeout(timer)
    // 恢复可见的瞬间重置基准：最小化期间 rAF 本来就不跑，不能当成「停摆」
    lastFrame = performance.now()

    timer = window.setTimeout(async () => {
      const stalledFor = performance.now() - lastFrame
      const zeroSize =
        document.documentElement.clientWidth === 0 ||
        document.documentElement.clientHeight === 0
      if (stalledFor < 1200 && !zeroSize) return

      console.warn(
        `[window-guard] 渲染疑似停摆（${reason}）：${Math.round(stalledFor)}ms 无帧，zeroSize=${zeroSize} → 请求强制重绘`,
      )
      try {
        await api.forceRepaintWindow()
      } catch (e) {
        console.warn('[window-guard] 强制重绘调用失败:', e)
      }
      window.dispatchEvent(new Event('resize'))

      window.setTimeout(() => {
        if (performance.now() - lastFrame <= 1200) {
          console.info('[window-guard] 强制重绘后已恢复出帧')
          // 让界面上出现一次提示，用户能直观确认「白屏自愈」生效
          window.dispatchEvent(new CustomEvent('wb:render-healed'))
          return
        }
        const now2 = Date.now()
        if (now2 - lastReloadAt < 10000) {
          console.error('[window-guard] 强制重绘无效，且 10 秒内已重载过，停止自动恢复')
          return
        }
        lastReloadAt = now2
        console.warn('[window-guard] 强制重绘无效 → 自动重载页面兜底')
        location.reload()
      }, 1200)
    }, 1200)
  }

  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible') void check('visibilitychange')
  })
  window.addEventListener('focus', () => void check('focus'))
  // 后端恢复窗口后主动通知（show_window 里 eval 的事件）
  window.addEventListener('wb:window-restored', () => {
    lastCheckAt = 0 // 后端主动修复的路径优先处理，跳过节流
    void check('wb:window-restored')
  })
}
