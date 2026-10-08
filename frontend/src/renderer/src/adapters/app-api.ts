// `window.api`：应用级能力。更新类与通知类走桌面外壳，其余走 daemon。

import { getLogger } from '../../../../packages/shared/logger/renderer.ts'
import { DEFERRED_CHANNELS } from './channel-map.ts'
import type { Backend } from './types.ts'

const logger = getLogger('app-api')

export interface AppApi {
  logToMain(...args: unknown[]): Promise<unknown>
  notification: { send(notification: unknown): Promise<unknown> }
  storeSync: {
    subscribe(): Promise<unknown>
    unsubscribe(): Promise<unknown>
    onUpdate(action: unknown): Promise<unknown>
  }
  onWindowShow(callback: (payload: unknown) => void): () => void
  onAppActivate(callback: (payload: unknown) => void): () => void
  /** 托盘菜单「屏幕监控」：点击后导航到屏幕监控页。 */
  onTrayNavigateToScreenMonitor(callback: () => void): () => void
  /** 托盘菜单「开始 / 暂停录制」。 */
  onTrayToggleRecording(callback: () => void): () => void
  checkForUpdate(): Promise<{ updateInfo: unknown }>
  quitAndInstall(): Promise<unknown>
  cancelDownload(): Promise<unknown>
}

/**
 * 外壳的事件订阅与命令调用能力。
 *
 * 由具体外壳提供（Tauri 实现见 `tauri-shell.ts`）：没有外壳时 `undefined`，
 * 相关能力退化为 no-op 并说明原因。
 */
export interface ShellCapabilities {
  listen?(channel: string, handler: (payload: unknown) => void): () => void
  invoke?(command: string, payload?: Record<string, unknown>): Promise<unknown>
}

/**
 * `shell` 提供外壳能力（更新/通知/窗口事件）。
 * 外壳能力由具体实现提供；这里只固定接口。
 */
export interface ShellBridge {
  checkForUpdate(): Promise<{ updateInfo: unknown }>
  quitAndInstall(): Promise<unknown>
  cancelDownload(): Promise<unknown>
  notify(notification: unknown): Promise<unknown>
  onWindowShow(callback: (payload: unknown) => void): () => void
  onAppActivate(callback: (payload: unknown) => void): () => void
}

// `backend` 目前没有直接用到的渠道（app 面全部由桌面外壳提供），
// 但保留这个参数：安装层的三个 create*Api 签名一致，接线时不用改调用点。
export function createAppApi(_backend: Backend, shell?: ShellBridge, shellCapabilities?: ShellCapabilities): AppApi {
  // 未接线时给出显式失败，而不是静默返回 undefined
  const notWired = (name: string) => async (): Promise<never> => {
    throw new Error(`${name} 需要桌面外壳（Tauri）提供，当前未接线`)
  }

  // 延后渠道是 no-op，但**不能是静默的**：第一次用到时把登记的原因说出来。
  const reported = new Set<string>()
  const reportDeferred = (channel: string): void => {
    if (reported.has(channel)) return
    reported.add(channel)
    const reason = DEFERRED_CHANNELS[channel] ?? '尚未实现'
    logger.warn(`[mc] ${channel} 当前是 no-op：${reason}`)
  }

  return {
    logToMain: async (...args) => {
      // 已登记为延后：日志改由 daemon 统一收集
      void DEFERRED_CHANNELS['app:log-to-main']
      logger.debug(...args)
      return undefined
    },

    notification: {
      send: shell ? (n) => shell.notify(n) : notWired('notification.send')
    },

    storeSync: {
      subscribe: async () => {
        reportDeferred('store-sync:subscribe')
        return undefined
      },
      unsubscribe: async () => {
        reportDeferred('store-sync:unsubscribe')
        return undefined
      },
      onUpdate: async () => {
        reportDeferred('store-sync:on-update')
        return undefined
      }
    },

    onWindowShow: (callback) => shell?.onWindowShow(callback) ?? (() => {}),
    onAppActivate: (callback) => shell?.onAppActivate(callback) ?? (() => {}),

    // 托盘菜单事件：走外壳事件订阅。没有外壳时返回空退订函数，不抛。
    onTrayNavigateToScreenMonitor: (callback) =>
      shellCapabilities?.listen?.('push:tray-navigate-to-screen-monitor', () => callback()) ?? (() => {}),
    onTrayToggleRecording: (callback) =>
      shellCapabilities?.listen?.('push:tray-toggle-recording', () => callback()) ?? (() => {}),

    checkForUpdate: shell ? () => shell.checkForUpdate() : notWired('checkForUpdate'),
    quitAndInstall: shell ? () => shell.quitAndInstall() : notWired('quitAndInstall'),
    cancelDownload: shell ? () => shell.cancelDownload() : notWired('cancelDownload')
  }
}
