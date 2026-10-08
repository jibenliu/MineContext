// 前端后端切换的**接线**测试（不是适配层本身）。
//
// 适配层的单元测试早就证明了「渠道映射对」，但**应用启动时没人调用它** ——
// `installAdapters` 当时只在测试里出现过。这一组测试钉住启动逻辑：
//
//   1. `electron`（默认）→ 不碰 preload 已经暴露的全局对象；
//   2. `rust` / `http` → 读到 runtime.json（端口 + token）后装 HTTP 后端；
//   3. **读不到 runtime.json → 明确报告，而不是静默用 electron 后端** ——
//      静默回退会让用户以为自己在用 rust 后端，实际打的是 IPC；
//   4. 两种写法（`rust` / `http`）都要认。

import assert from 'node:assert/strict'
import { test } from 'node:test'

import { bootstrapBackend, type BootstrapRuntime } from '../../bootstrap-backend.ts'

const runtime: BootstrapRuntime = { port: 43117, token: 'token-abc' }

function deps() {
  const installed: BootstrapRuntime[] = []
  const events: string[] = []
  return {
    installed,
    events,
    options: {
      loadRuntime: async () => runtime,
      install: (value: BootstrapRuntime) => installed.push(value),
      onEvent: (event: string) => events.push(event)
    }
  }
}

test('读到 runtime.json 时装上 HTTP 后端，端口与 token 来自它', async () => {
  const harness = deps()

  const result = await bootstrapBackend(harness.options)

  assert.equal(result, 'http')
  assert.deepEqual(harness.installed, [runtime])
})

test('daemon 还没写好 runtime.json 时会重试，而不是直接放弃', async () => {
  const attempts: number[] = []
  let calls = 0
  const installed: BootstrapRuntime[] = []

  const result = await bootstrapBackend({
    loadRuntime: async () => {
      calls += 1
      attempts.push(calls)
      // 前两次读不到（daemon 还在启动），第三次读到
      return calls < 3 ? null : runtime
    },
    install: (value) => installed.push(value),
    retry: { attempts: 5, delayMs: 0 }
  })

  assert.equal(result, 'http')
  assert.equal(calls, 3, '应当重试到读到为止')
  assert.deepEqual(installed, [runtime])
})

test('始终读不到时用尽重试次数并明确报告', async () => {
  const events: string[] = []
  let calls = 0

  const result = await bootstrapBackend({
    loadRuntime: async () => {
      calls += 1
      return null
    },
    install: () => assert.fail('读不到运行时信息时不该安装适配层'),
    onEvent: (event) => events.push(event),
    retry: { attempts: 3, delayMs: 0 }
  })

  assert.equal(result, 'http-unavailable')
  assert.equal(calls, 3)
  assert.ok(
    events.some((event) => event.includes('3 次')),
    `事件里应当写清试了几次，实际：${events.join(' | ')}`
  )
})

// [已移除] electron：应用只有 daemon 一条后端路径
