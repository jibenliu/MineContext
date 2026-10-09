// 渲染层 → 外壳 `renderer_log` 落盘。
//
// 必须在 bootstrap **之前**装上：以前只在 `installHttpBackendFromRuntime` 成功后
// 才 `setLogSink`，打包版一旦 get_runtime 失败，界面能画出「无法连接本地服务」，
// 但 renderer.log mtime 完全不动 —— 与用户复现（daemon.log 增长、renderer.log 冻结）一致。

import { setLogSink } from '../../../../packages/shared/logger/renderer.ts'
import { createTauriShellCapabilities } from './tauri-shell.ts'

/** 探测到 Tauri invoke 就挂上落盘出口；没有外壳则卸掉。 */
export function installRendererLogSink(target: unknown = globalThis): boolean {
  const capabilities = createTauriShellCapabilities(target)
  const forward = capabilities?.invoke
  if (!forward) {
    setLogSink(undefined)
    return false
  }
  setLogSink((level, message) => {
    void forward('renderer_log', { level, message }).catch(() => undefined)
  })
  return true
}
