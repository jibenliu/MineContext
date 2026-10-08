// Tauri 外壳桥：把外壳能力（通知/更新/窗口事件）接到 Tauri 命令上。
//
// 能力来自 `window.mcRuntime.shell`（Tauri 初始化脚本注入）。没声明能力时
// **明确失败**而不是静默无操作：静默会让「通知没弹出来」变成查不动的问题。

import type { ShellBridge, ShellCapabilities } from './app-api.ts'

/** Tauri 注入的全局形状（只取这里用到的字段）。 */
export interface TauriGlobals {
  __TAURI__?: {
    core?: { invoke?: (command: string, args?: Record<string, unknown>) => Promise<unknown> }
    event?: {
      listen?: (channel: string, handler: (event: { payload?: unknown }) => void) => Promise<() => void>
    }
  }
  mcRuntime?: {
    shell?: { notification?: boolean }
  }
}

/**
 * 外壳事件与命令的通用桥：给 `ipc-renderer-shim` 用（托盘事件、托盘状态上报）。
 *
 * `event.listen` 返回的是 Promise，而 shim 需要同步拿到取消订阅函数：这里先把
 * 句柄接住，晚到的 unlisten 在已经取消的情况下立刻调用，避免漏掉订阅。
 */
export function createTauriShellCapabilities(target: unknown): ShellCapabilities | undefined {
  const globals = (target ?? {}) as TauriGlobals
  const invoke = globals.__TAURI__?.core?.invoke
  const listen = globals.__TAURI__?.event?.listen
  if (typeof invoke !== 'function' && typeof listen !== 'function') return undefined

  const capabilities: ShellCapabilities = {}

  if (typeof invoke === 'function') {
    capabilities.invoke = (command, payload) => invoke(command, payload)
  }

  if (typeof listen === 'function') {
    capabilities.listen = (channel, handler) => {
      let unlisten: (() => void) | undefined
      let cancelled = false
      void listen(channel, (event) => handler(event?.payload)).then((off) => {
        if (cancelled) {
          off()
          return
        }
        unlisten = off
      })
      return () => {
        cancelled = true
        unlisten?.()
      }
    }
  }

  return capabilities
}

function reject(name: string, reason: string): () => Promise<never> {
  return async () => {
    throw new Error(`${name} 不可用：${reason}`)
  }
}

/**
 * 探测并构造 Tauri 外壳桥。非 Tauri 环境返回 `undefined`，
 * 让调用方继续用 `window.api` 提供的真桥。
 */
export function createTauriShellBridge(target: unknown): ShellBridge | undefined {
  const globals = (target ?? {}) as TauriGlobals
  const invoke = globals.__TAURI__?.core?.invoke
  if (typeof invoke !== 'function') return undefined

  const capabilities = globals.mcRuntime?.shell ?? {}
  const updater = reject('更新', '当前外壳未接线（需要签名与公证）')

  return {
    notify: capabilities.notification
      ? async (notification) => {
          const item = (notification ?? {}) as { title?: string; message?: string }
          await invoke('plugin:notification|notify', {
            options: { title: item.title ?? '', body: item.message ?? '' }
          })
          return undefined
        }
      : reject('系统通知', '当前外壳未声明 notification 能力'),

    checkForUpdate: async () => {
      await updater()
      return { updateInfo: null }
    },
    quitAndInstall: updater,
    cancelDownload: updater,

    // Tauri 外壳不提供窗口显示/激活事件：返回解绑函数，保持接口形状一致。
    onWindowShow: () => () => {},
    onAppActivate: () => () => {}
  }
}
