// Tauri 外壳桥：能力声明决定成功还是明确失败。用 node --test 跑。

import assert from 'node:assert/strict'
import { test } from 'node:test'

import { createTauriShellBridge, createTauriShellCapabilities } from '../tauri-shell.ts'

type Call = { command: string; args?: Record<string, unknown> }

function fakeTauri(shell?: { notification?: boolean }): { calls: Call[]; target: unknown } {
  const calls: Call[] = []
  return {
    calls,
    target: {
      __TAURI__: {
        core: {
          invoke: async (command: string, args?: Record<string, unknown>) => {
            calls.push({ command, args })
            return undefined
          }
        }
      },
      mcRuntime: { shell }
    }
  }
}

test('非 Tauri 环境返回 undefined，让调用方继续用真桥', () => {
  assert.equal(createTauriShellBridge(undefined), undefined)
  assert.equal(createTauriShellBridge({}), undefined)
  // 形状不完整（有 __TAURI__ 但没有 invoke）时同样不接管
  assert.equal(createTauriShellBridge({ __TAURI__: { core: {} } }), undefined)
})

test('声明 notification 能力时，通知走 Tauri 插件命令', async () => {
  const { calls, target } = fakeTauri({ notification: true })
  const bridge = createTauriShellBridge(target)
  assert.ok(bridge)

  await bridge.notify({ title: '标题', message: '正文' })

  assert.equal(calls.length, 1)
  assert.equal(calls[0].command, 'plugin:notification|notify')
  assert.deepEqual(calls[0].args, { options: { title: '标题', body: '正文' } })
})

test('未声明 notification 能力时明确失败，且不调用任何命令', async () => {
  const { calls, target } = fakeTauri({})
  const bridge = createTauriShellBridge(target)
  assert.ok(bridge)

  await assert.rejects(() => bridge.notify({ title: 'a', message: 'b' }), /系统通知 不可用/)
  assert.equal(calls.length, 0)
})

test('更新类命令明确失败（外壳未接线），并说明原因', async () => {
  const { target } = fakeTauri({ notification: true })
  const bridge = createTauriShellBridge(target)
  assert.ok(bridge)

  await assert.rejects(() => bridge.checkForUpdate(), /更新 不可用：当前外壳未接线/)
  await assert.rejects(() => bridge.quitAndInstall(), /更新 不可用/)
  await assert.rejects(() => bridge.cancelDownload(), /更新 不可用/)
})

test('窗口事件返回解绑函数，接口形状与 Electron 桥一致', () => {
  const { target } = fakeTauri({})
  const bridge = createTauriShellBridge(target)
  assert.ok(bridge)

  assert.equal(typeof bridge.onWindowShow(() => {}), 'function')
  assert.equal(typeof bridge.onAppActivate(() => {}), 'function')
})

// ---- 托盘事件与托盘状态上报：外壳事件/命令的通用桥 ----

type ListenCall = { channel: string; payload: unknown }
type UnlistenCall = { channel: string; off: () => void }

function fakeShellEvents(): {
  listens: ListenCall[]
  offs: UnlistenCall[]
  target: unknown
  emit(channel: string, payload: unknown): void
} {
  const listeners = new Map<string, (event: { payload?: unknown }) => void>()
  const listens: ListenCall[] = []
  const offs: UnlistenCall[] = []
  return {
    listens,
    offs,
    target: {
      __TAURI__: {
        event: {
          listen: async (channel: string, handler: (event: { payload?: unknown }) => void) => {
            listeners.set(channel, handler)
            listens.push({ channel, payload: undefined })
            const off = () => offs.push({ channel, off })
            return off
          }
        }
      }
    },
    emit(channel, payload) {
      listeners.get(channel)?.({ payload })
    }
  }
}

test('托盘事件走外壳 event.listen，payload 解包后交给监听者', async () => {
  const fake = fakeShellEvents()
  const capabilities = createTauriShellCapabilities(fake.target)
  assert.ok(capabilities?.listen)

  const received: unknown[] = []
  capabilities.listen('push:tray-toggle-recording', (payload) => received.push(payload))
  // listen 是异步的：给它一个宏任务把内部 unlisten 句柄接住
  await Promise.resolve()
  fake.emit('push:tray-toggle-recording', { at: 1 })

  assert.deepEqual(received, [{ at: 1 }])
  assert.equal(fake.listens.length, 1, '应当只订阅一次')
})

test('取消订阅发生在 listen 兑现之前时，晚到的 unlisten 会被立刻调用', async () => {
  const fake = fakeShellEvents()
  const capabilities = createTauriShellCapabilities(fake.target)
  assert.ok(capabilities?.listen)

  const off = capabilities.listen('push:tray-navigate-to-screen-monitor', () => {})
  off()
  await Promise.resolve()
  await Promise.resolve()

  assert.equal(fake.offs.length, 1, '晚到的 unlisten 必须被调用，否则会漏掉订阅')
})

test('没有 __TAURI__ 时返回 undefined（调用方据此走"未接线"告警）', async () => {
  assert.equal(createTauriShellCapabilities(undefined), undefined)
  assert.equal(createTauriShellCapabilities({}), undefined)
})
