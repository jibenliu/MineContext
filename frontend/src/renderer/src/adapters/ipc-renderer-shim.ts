// `window.electron.ipcRenderer` 兼容层。
//
// Router.tsx 与 screen-monitor.tsx 直接调用 ipcRenderer.on('push:tray-...')。
// 有了这层 shim，这些文件不必知道外壳换成了什么实现。
//
// 没有实现的渠道在这里**说清楚**：既没有 HTTP 映射、也没有推送来源的渠道，
// 直接订阅会得到一个永远不触发的监听器 —— 那是最难查的一类问题。凡是走这条
// 路径的渠道，第一次用到时在控制台写一条带原因的告警，然后按「延后」处理。

import { getLogger } from '../../../../packages/shared/logger/renderer.ts'
import { DEFERRED_CHANNELS, SHELL_PUSH_CHANNELS, subscriptionEvent } from './channel-map.ts'
import type { Backend } from './types.ts'

const logger = getLogger('ipc-renderer-shim')

/** 监听者签名与桌面外壳渠道一致：第一个参数是事件对象，后面是 payload。 */
type Listener = (...args: any[]) => void

/** 外壳提供的能力：推送订阅与命令调用。没有它时相关渠道只能登记为「未接线」。 */
export interface ShellCapabilities {
  listen?(channel: string, handler: (payload: unknown) => void): () => void
  invoke?(command: string, payload?: Record<string, unknown>): Promise<unknown>
}

/** 由外壳命令实现的渠道：命令名与参数翻译只写在这里，业务代码不需要知道。 */
const SHELL_COMMANDS: Record<string, (args: unknown[]) => { command: string; payload: Record<string, unknown> }> = {
  // 屏幕监控页在录制状态变化时上报，用来更新托盘的提示与菜单文案
  'tray:update-recording-status': (args) => ({
    command: 'tray_recording_status',
    payload: { recording: Boolean(args[0]) }
  })
}

export interface IpcRendererShim {
  invoke(channel: string, ...args: unknown[]): Promise<unknown>
  send(channel: string, ...args: unknown[]): void
  /** 订阅并返回取消订阅函数（比依赖 `this` 更好用）。 */
  on(channel: string, listener: Listener): () => void
  once(channel: string, listener: Listener): () => void
  removeListener(channel: string, listener: Listener): void
  removeAllListeners(channel: string): void
  off(channel: string, listener: Listener): void
}

export function createIpcRendererShim(backend: Backend, shell?: ShellCapabilities): IpcRendererShim {
  // 渠道 → 原始监听者 → 取消订阅函数。
  // 需要保留原始引用，才能让 removeListener 正确匹配（once 会用包装函数）。
  const registrations = new Map<string, Map<Listener, () => void>>()

  // 同一个渠道只说一次：反复刷屏会把真正的问题淹掉。
  const reported = new Set<string>()

  function reportOnce(channel: string, reason: string): void {
    if (reported.has(channel)) return
    reported.add(channel)
    logger.warn(`[mc] 渠道 ${channel} 当前没有实现：${reason}`)
  }

  function deferReason(channel: string): string | undefined {
    return DEFERRED_CHANNELS[channel]
  }

  function registrationsFor(channel: string): Map<Listener, () => void> {
    const existing = registrations.get(channel)
    if (existing) return existing
    const created = new Map<Listener, () => void>()
    registrations.set(channel, created)
    return created
  }

  /** 登记监听者；`subscribe` 为空表示这个渠道没有后端推送来源。 */
  function add(channel: string, listener: Listener, subscribe: boolean): () => void {
    const map = registrationsFor(channel)
    const existing = map.get(listener)
    if (existing) return existing

    // 推送回调签名是 (event, payload...)，业务代码取 payload。
    const unsubscribe = subscribe ? backend.subscribe(channel, (payload) => listener({}, payload)) : () => undefined
    map.set(listener, unsubscribe)
    return unsubscribe
  }

  /** 外壳推送的渠道：订阅走外壳事件，而不是 daemon 的 SSE。 */
  function attachShellPush(channel: string, listener: Listener, reason: string): () => void {
    const listen = shell?.listen
    if (!listen) {
      reportOnce(channel, `${reason}；当前外壳未接线（没有 __TAURI__），订阅不会收到事件`)
      return add(channel, listener, false)
    }
    const map = registrationsFor(channel)
    const existing = map.get(listener)
    if (existing) return existing
    const unsubscribe = listen(channel, (payload) => listener({}, payload))
    map.set(listener, unsubscribe)
    return unsubscribe
  }

  /** 订阅入口：没有推送来源的渠道先说清楚，再登记一个永远不会触发的监听者。 */
  function attach(channel: string, listener: Listener): () => void {
    const fromShell = SHELL_PUSH_CHANNELS[channel]
    if (fromShell) return attachShellPush(channel, listener, fromShell)

    const deferred = deferReason(channel)
    if (deferred) {
      reportOnce(channel, deferred)
      return add(channel, listener, false)
    }
    if (subscriptionEvent(channel) === undefined) {
      reportOnce(channel, '既没有 HTTP 映射，也没有 SSE 事件名')
      return add(channel, listener, false)
    }
    return add(channel, listener, true)
  }

  function remove(channel: string, listener: Listener): void {
    const map = registrations.get(channel)
    const unsubscribe = map?.get(listener)
    if (!unsubscribe) return
    unsubscribe()
    map?.delete(listener)
  }

  /** 退订一个渠道上的全部监听者（`App.tsx` 卸载时用它）。 */
  function removeAll(channel: string): void {
    const map = registrations.get(channel)
    if (!map) return
    for (const unsubscribe of map.values()) unsubscribe()
    registrations.delete(channel)
  }

  return {
    invoke: (channel, ...args) => {
      const command = SHELL_COMMANDS[channel]
      if (command) {
        if (!shell?.invoke) {
          reportOnce(channel, '由桌面外壳的命令实现；当前外壳未接线，调用被忽略')
          return Promise.resolve(undefined)
        }
        const { command: name, payload } = command(args)
        return shell.invoke(name, payload)
      }
      const reason = deferReason(channel)
      if (reason) {
        reportOnce(channel, reason)
        return Promise.resolve(undefined)
      }
      return backend.invoke(channel, ...args)
    },
    send: (channel, ...args) => {
      // 旧 ipcRenderer.send 是「发了不管」，这里同样不阻塞调用方。
      const reason = deferReason(channel)
      if (reason) {
        reportOnce(channel, reason)
        return
      }
      void backend.invoke(channel, ...args)
    },
    on: (channel, listener) => attach(channel, listener),
    once: (channel, listener) => {
      const wrapper: Listener = (...args) => {
        remove(channel, wrapper)
        listener(...args)
      }
      return attach(channel, wrapper)
    },
    removeListener: remove,
    removeAllListeners: removeAll,
    off: remove
  }
}
