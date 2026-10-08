// 页面级测试的公共环境补丁与工具。
//
// jsdom 里没有 `matchMedia` / `ResizeObserver` / `scrollTo`，而 Arco 组件与
// allotment 布局在挂载时会用到它们 —— 缺一个就是「组件没渲染出来」这种
// 与业务无关的失败。这里统一补上，测试文件只关心自己的断言。
//
// 另外提供一个**假后端**：页面组件通过 `window.dbAPI` / `window.serverPushAPI`
// 取数，测试只需要给出「每个渠道返回什么」与「手动推一条事件」。

import '@testing-library/jest-dom/vitest'

import { installAdapters } from '@renderer/adapters/install'
import type { Backend } from '@renderer/adapters/types'
import { configureHttpClient } from '@renderer/services/axios-config'
import { cleanup } from '@testing-library/react'
import { afterEach, vi } from 'vitest'

if (!window.matchMedia) {
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false
  })) as unknown as typeof window.matchMedia
}

if (!globalThis.ResizeObserver) {
  globalThis.ResizeObserver = class {
    observe() {
      // jsdom 没有布局：观察不到任何尺寸变化
    }
    unobserve() {
      // 同上
    }
    disconnect() {
      // 同上
    }
  } as unknown as typeof ResizeObserver
}

window.HTMLElement.prototype.scrollTo = () => {}

afterEach(() => {
  cleanup()
})

installFakeBackend({ 'screen-monitor:get-capture-all-sources': { success: true, sources: [] } }, { strict: false })

/** 每个渠道返回什么的假后端；`push` 用来手动触发推送。 */
export interface FakeBackend extends Backend {
  push(event: string, payload: unknown): void
  calls: Array<{ channel: string; args: unknown[] }>
}

export interface FakeBackendOptions {
  /**
   * 严格模式（默认）：没配置的渠道直接抛错。
   * 非严格模式：没配置的渠道返回**空数组** —— 用测「daemon 起来了但还没有数据」
   * 的页面空态（切换后端时用户真会看到的第一个界面）。
   *
   * 空数组而不是 `null`：页面里有 `res.length`、`get(res, 'x', [])` 这类写法，
   * 给 `null` 会在 effect 里抛未处理的 rejection（页面看起来「空白」而测试仍绿）。
   */
  strict?: boolean
}

export function fakeBackend(handlers: Record<string, unknown> = {}, options: FakeBackendOptions = {}): FakeBackend {
  const strict = options.strict ?? true
  const subscribers = new Map<string, Set<(payload: unknown) => void>>()
  const calls: Array<{ channel: string; args: unknown[] }> = []

  return {
    kind: 'http',
    calls,
    async invoke<T>(channel: string, ...args: unknown[]): Promise<T> {
      calls.push({ channel, args })
      if (!(channel in handlers)) {
        if (strict) {
          throw new Error(`假后端没有为 ${channel} 配置返回值`)
        }
        return [] as unknown as T
      }
      return handlers[channel] as T
    },
    subscribe(event: string, handler: (payload: unknown) => void) {
      const set = subscribers.get(event) ?? new Set()
      set.add(handler)
      subscribers.set(event, set)
      return () => set.delete(handler)
    },
    push(event: string, payload: unknown) {
      for (const handler of subscribers.get(event) ?? []) {
        handler(payload)
      }
    }
  }
}

/** 把适配层装到 jsdom 的 `window` 上，返回假后端供测试驱动。 */
export function installFakeBackend(
  handlers: Record<string, unknown> = {},
  options: FakeBackendOptions = {}
): FakeBackend {
  const backend = fakeBackend(handlers, options)
  installAdapters({ backend, target: window as unknown as Record<string, unknown> })
  // 给业务层 axios 一个占位地址：假后端环境里既没有 daemon 也没有 runtime.json，
  // 不指的话直接走 axios 的路径（对话流、设置页）会报「还没读到 daemon 的 runtime.json」，
  // 那句话在这里纯属误导。地址不会被真请求到 —— 假后端按渠道返回、网络调用各自打桩。
  configureHttpClient(0, 'fake-backend-token')
  return backend
}

/** 页面真的通过适配层取数了吗？（空态测试也要证明「接线了」） */
export function calledChannels(backend: FakeBackend): string[] {
  return [...new Set(backend.calls.map((call) => call.channel))].sort()
}

export { vi }
