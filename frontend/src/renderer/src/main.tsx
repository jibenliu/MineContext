// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import '@arco-design/web-react/es/_util/react-19-adapter'

import { getLogger } from '@shared/logger/renderer'
import { createRoot } from 'react-dom/client'

import { installHttpBackendFromRuntime } from './adapters/install'
import App from './app'
import { bootstrapBackend } from './bootstrap-backend'
import { installDevStandalone } from './dev-standalone'
import { getLocale, translate } from './i18n'
import { removeStartupSpinner } from './utils/startup-spinner'

const logger = getLogger('main')

// 决定后端并（有 daemon 时）装好适配层，**再**渲染：
// 页面组件在挂载时就会取数，
// 先渲染再装会出现「第一次请求打到不存在的后端」。
//
// 两条兜底都是为了「不许白屏」：
// 1. 读运行时信息失败（外壳桥没接线）不算致命 —— 交给 bootstrap 重试，最后如实
//    报告「后端不可用」，界面显示原因；
// 2. bootstrap 本身抛异常时，页面上直接写出原因 —— 打包版看不到控制台，白屏等于
//    什么都查不到。
//
// `#spinner` **不要**在 createRoot 后立刻拆：`PersistGate` 在 rehydrate 完成前
// 子树是空的（loading={null}）。过早拆掉占位 = Tauri WKWebView 里一段真白屏。
// 由 `AppContent` 首帧挂载后再 `removeStartupSpinner`。
void (async () => {
  try {
    // 纯浏览器开发（`pnpm dev`，没有 Tauri 外壳）：装 mock 后端并直接进入界面。
    // 生产构建会把 `import.meta.env.DEV` 折为 false，此处整段（含桩模块）被摇出。
    let backendReady = false
    if (import.meta.env.DEV && !(window as { __TAURI__?: unknown }).__TAURI__) {
      installDevStandalone()
      backendReady = true
    } else {
      const result = await bootstrapBackend({
        loadRuntime: async () => {
          if (typeof window.mcRuntime?.get !== 'function') {
            logger.warn('[mc] window.mcRuntime.get 未注入（初始化脚本/外壳桥未接线）')
            return null
          }
          try {
            const runtime = await window.mcRuntime.get()
            return runtime ?? null
          } catch (error) {
            // 常见于 Tauri 2 ACL 未放行 get_runtime：daemon 已听端口，invoke 仍被拒。
            logger.error('[mc] 读取运行时信息失败（get_runtime invoke）：', error)
            return null
          }
        },
        install: (runtime) => installHttpBackendFromRuntime(runtime),
        onEvent: (event) => logger.info('[mc]', event)
      })
      backendReady = result === 'http'
    }

    createRoot(document.getElementById('root')!).render(<App backendReady={backendReady} />)
  } catch (error) {
    renderStartupFailure(error)
  }
})()

/** 启动失败时的最小可见反馈：不依赖适配层，也不依赖 React。 */
function renderStartupFailure(error: unknown): void {
  logger.error('[mc] 启动失败：', error)
  removeStartupSpinner()
  const root = document.getElementById('root')
  if (!root) return
  const box = document.createElement('div')
  box.style.cssText = 'padding:24px;font:14px/1.6 -apple-system,sans-serif;color:#3F3F51'
  // 启动早期语言可能还没就绪：取词失败就退回默认文案，不让兜底页自己崩。
  let headline = 'MineContext 启动失败'
  try {
    headline = translate(getLocale(), 'app.startupFailed')
  } catch {
    // 保持默认
  }
  box.textContent = `${headline}：${error instanceof Error ? error.message : String(error)}`
  root.replaceChildren(box)
}
