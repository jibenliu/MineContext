// `window.electron.ipcRenderer` 兼容层。
//
// Router.tsx 与 screen-monitor.tsx 直接调用 ipcRenderer.on(...) 监听托盘事件。
// 有了这层 shim，这些文件在切到 Tauri 时**一行都不用改**。
//
// 另一半同样重要：没有后端来源的渠道不能静默变成「永远不会触发的监听器」，
// 第一次用到时必须留下一条带原因的告警。

import assert from 'node:assert/strict'
import { test } from 'node:test'

import { createIpcRendererShim } from '../ipc-renderer-shim.ts'
import type { Backend } from '../types.ts'

function fakeBackend() {
  const handlers = new Map<string, Set<(payload: unknown) => void>>()
  const invokes: { channel: string; args: unknown[] }[] = []

  const backend: Backend = {
    kind: 'http',
    async invoke<T>(channel: string, ...args: unknown[]): Promise<T> {
      invokes.push({ channel, args })
      return undefined as T
    },
    subscribe(event, handler) {
      const set = handlers.get(event) ?? new Set()
      set.add(handler)
      handlers.set(event, set)
      return () => set.delete(handler)
    }
  }

  return {
    backend,
    invokes,
    emit(event: string, payload: unknown) {
      for (const handler of handlers.get(event) ?? []) handler(payload)
    },
    listenerCount(event: string) {
      return handlers.get(event)?.size ?? 0
    }
  }
}

/** 捕获 console.warn：告警是这层的对外行为之一，必须能被断言。 */
function captureWarnings(): { messages: string[]; restore: () => void } {
  const original = console.warn
  const messages: string[] = []
  console.warn = (...args: unknown[]) => {
    messages.push(args.map((arg) => String(arg)).join(' '))
  }
  return {
    messages,
    restore: () => {
      console.warn = original
    }
  }
}

test('sse_push_reaches_listener_via_shim', () => {
  const fake = fakeBackend()
  const ipcRenderer = createIpcRendererShim(fake.backend)

  // 每次回调收到的是 Electron 的 `(event, ...args)`，因此这里是「参数数组的数组」
  const received: unknown[][] = []
  ipcRenderer.on('push:screen-monitor-status', (...args: unknown[]) => {
    received.push(args)
  })

  fake.emit('push:screen-monitor-status', { at: 1 })

  assert.equal(received.length, 1, 'daemon 的推送没有送到监听者')
  // 保持 Electron 的 (event, ...args) 约定：args[0] 是事件对象，payload 在 args[1]。
  assert.equal(typeof received[0][0], 'object', 'args[0] 应为事件对象')
  assert.deepEqual(received[0][1], { at: 1 })
})

// 托盘动作在 Tauri 外壳里还没有实现：订阅它必须留下告警，而不是变成哑监听器
test('deferred_channel_warns_with_reason_and_does_not_subscribe_backend', () => {
  const fake = fakeBackend()
  const ipcRenderer = createIpcRendererShim(fake.backend)
  const warnings = captureWarnings()
  try {
    let calls = 0
    ipcRenderer.on('push:tray-toggle-recording', () => {
      calls += 1
    })

    fake.emit('push:tray-toggle-recording', undefined)

    assert.equal(calls, 0, '没有实现的渠道不该被后端触发')
    assert.equal(fake.listenerCount('push:tray-toggle-recording'), 0, '不该去后端订阅')
    assert.equal(warnings.messages.length, 1, '应当告警一次')
    assert.match(warnings.messages[0], /push:tray-toggle-recording/)
    assert.match(warnings.messages[0], /托盘/)
  } finally {
    warnings.restore()
  }
})

test('deferred_channel_warns_only_once', () => {
  const fake = fakeBackend()
  const ipcRenderer = createIpcRendererShim(fake.backend)
  const warnings = captureWarnings()
  try {
    ipcRenderer.on('push:tray-navigate-to-screen-monitor', () => {})
    ipcRenderer.on('push:tray-navigate-to-screen-monitor', () => {})

    assert.equal(warnings.messages.length, 1, '同一个渠道只该告警一次')
  } finally {
    warnings.restore()
  }
})

test('listener_registered_for_deferred_channel_can_still_be_removed', () => {
  const fake = fakeBackend()
  const ipcRenderer = createIpcRendererShim(fake.backend)
  const warnings = captureWarnings()
  try {
    const handler = () => {}
    ipcRenderer.on('push:tray-toggle-recording', handler)
    ipcRenderer.removeListener('push:tray-toggle-recording', handler)
    ipcRenderer.off('push:tray-toggle-recording', handler)
  } finally {
    warnings.restore()
  }
})

test('deferred_invoke_resolves_without_hitting_backend', async () => {
  const fake = fakeBackend()
  const ipcRenderer = createIpcRendererShim(fake.backend)
  const warnings = captureWarnings()
  try {
    const result = await ipcRenderer.invoke('tray:update-recording-status', true)

    assert.equal(result, undefined)
    assert.deepEqual(fake.invokes, [], '延后渠道不该发 HTTP 请求')
    assert.equal(warnings.messages.length, 1)
    assert.match(warnings.messages[0], /tray:update-recording-status/)
  } finally {
    warnings.restore()
  }
})

test('zero_argument_listener_is_still_invoked', () => {
  const fake = fakeBackend()
  const ipcRenderer = createIpcRendererShim(fake.backend)

  let calls = 0
  ipcRenderer.on('push:screen-monitor-status', () => {
    calls += 1
  })

  fake.emit('push:screen-monitor-status', undefined)
  fake.emit('push:screen-monitor-status', undefined)

  assert.equal(calls, 2)
})

test('shim_invoke_forwards_channel_and_args', async () => {
  const fake = fakeBackend()
  const ipcRenderer = createIpcRendererShim(fake.backend)

  await ipcRenderer.invoke('backend:get-status', 'x', 2)

  assert.deepEqual(fake.invokes, [{ channel: 'backend:get-status', args: ['x', 2] }])
})

test('shim_off_removes_listener', () => {
  const fake = fakeBackend()
  const ipcRenderer = createIpcRendererShim(fake.backend)

  const calls: unknown[] = []
  const handler = (...args: unknown[]) => calls.push(args[0])

  ipcRenderer.on('push:screen-monitor-status', handler)
  assert.equal(fake.listenerCount('push:screen-monitor-status'), 1)

  ipcRenderer.removeListener('push:screen-monitor-status', handler)
  fake.emit('push:screen-monitor-status', 'running')

  assert.deepEqual(calls, [], '移除后不应再收到事件')
})

test('shim_once_fires_exactly_once', () => {
  const fake = fakeBackend()
  const ipcRenderer = createIpcRendererShim(fake.backend)

  let count = 0
  ipcRenderer.once('push:latest-activity', () => {
    count += 1
  })

  fake.emit('push:latest-activity', 1)
  fake.emit('push:latest-activity', 2)

  assert.equal(count, 1)
})

test('shim_send_forwards_like_invoke', () => {
  const fake = fakeBackend()
  const ipcRenderer = createIpcRendererShim(fake.backend)

  ipcRenderer.send('backend:get-status', 'running')

  assert.deepEqual(fake.invokes[0], {
    channel: 'backend:get-status',
    args: ['running']
  })
})

// ---- 外壳提供的能力：托盘事件与托盘状态上报 ----

/** 假外壳：记录 listen/invoke，并能手动推一条事件。 */
function fakeShell(): {
  listens: string[]
  invokes: { command: string; payload?: Record<string, unknown> }[]
  offs: number
  capabilities: {
    listen: (channel: string, handler: (payload: unknown) => void) => () => void
    invoke: (command: string, payload?: Record<string, unknown>) => Promise<unknown>
  }
  emit(channel: string, payload: unknown): void
} {
  const handlers = new Map<string, (payload: unknown) => void>()
  const listens: string[] = []
  const invokes: { command: string; payload?: Record<string, unknown> }[] = []
  const state = { offs: 0 }
  return {
    listens,
    invokes,
    get offs() {
      return state.offs
    },
    capabilities: {
      listen: (channel, handler) => {
        listens.push(channel)
        handlers.set(channel, handler)
        return () => {
          state.offs += 1
          handlers.delete(channel)
        }
      },
      invoke: async (command, payload) => {
        invokes.push({ command, payload })
        return 'ok'
      }
    },
    emit(channel, payload) {
      handlers.get(channel)?.(payload)
    }
  }
}

test('托盘事件由外壳推送时能到达监听者（不再需要 daemon SSE）', () => {
  const fake = fakeBackend()
  const shell = fakeShell()
  const ipcRenderer = createIpcRendererShim(fake.backend, shell.capabilities)

  const received: unknown[][] = []
  ipcRenderer.on('push:tray-toggle-recording', (...args: unknown[]) => received.push(args))

  shell.emit('push:tray-toggle-recording', undefined)

  assert.equal(received.length, 1, '托盘菜单的事件必须送到监听者')
  assert.deepEqual(received[0], [{}, undefined], '保持 (event, payload) 的旧签名')
  assert.deepEqual(fake.invokes, [], '托盘事件不该走后端 HTTP')
  assert.equal(fake.listenerCount('push:tray-toggle-recording'), 0, '也不该订阅 daemon SSE')
})

test('托盘事件的取消订阅会传到外壳', () => {
  const fake = fakeBackend()
  const shell = fakeShell()
  const ipcRenderer = createIpcRendererShim(fake.backend, shell.capabilities)

  const handler = () => {}
  ipcRenderer.on('push:tray-navigate-to-screen-monitor', handler)
  ipcRenderer.removeListener('push:tray-navigate-to-screen-monitor', handler)

  assert.equal(shell.offs, 1, '取消订阅必须传下去，否则外壳侧会留下死监听')
})

test('托盘状态上报翻译成外壳命令（带参数）', async () => {
  const fake = fakeBackend()
  const shell = fakeShell()
  const ipcRenderer = createIpcRendererShim(fake.backend, shell.capabilities)

  const result = await ipcRenderer.invoke('tray:update-recording-status', {
    recording: true,
    tooltip: 'MineContext · Recording',
    toggleLabel: 'Stop Recording',
    title: 'R'
  })

  assert.equal(result, 'ok')
  assert.deepEqual(shell.invokes, [
    {
      command: 'tray_recording_status',
      payload: {
        recording: true,
        tooltip: 'MineContext · Recording',
        toggle_label: 'Stop Recording',
        title: 'R'
      }
    }
  ])
  assert.deepEqual(fake.invokes, [], '外壳渠道不该发到 daemon')
})

test('托盘状态上报兼容旧的布尔参数', async () => {
  const fake = fakeBackend()
  const shell = fakeShell()
  const ipcRenderer = createIpcRendererShim(fake.backend, shell.capabilities)

  await ipcRenderer.invoke('tray:update-recording-status', true)

  assert.deepEqual(shell.invokes, [
    {
      command: 'tray_recording_status',
      payload: { recording: true, tooltip: '', toggle_label: '', title: '' }
    }
  ])
})

test('没有外壳时托盘渠道告警一次并忽略调用（不静默、不发错请求）', async () => {
  const fake = fakeBackend()
  const ipcRenderer = createIpcRendererShim(fake.backend)
  const warnings = captureWarnings()
  try {
    const result = await ipcRenderer.invoke('tray:update-recording-status', true)
    ipcRenderer.on('push:tray-toggle-recording', () => {})

    assert.equal(result, undefined)
    assert.deepEqual(fake.invokes, [])
    assert.equal(warnings.messages.length, 2, '两个渠道各告警一次')
    assert.match(warnings.messages[0], /外壳/)
    assert.match(warnings.messages[1], /托盘菜单/)
  } finally {
    warnings.restore()
  }
})
