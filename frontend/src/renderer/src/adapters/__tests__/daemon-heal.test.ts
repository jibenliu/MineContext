import assert from 'node:assert/strict'
import { test } from 'node:test'

import { reclaimBackend, runtimeIdentityChanged } from '../daemon-heal.ts'
import type { BootstrapRuntime } from '../../bootstrap-backend.ts'

const previous: BootstrapRuntime = { port: 43117, token: 'old' }
const next: BootstrapRuntime = { port: 43118, token: 'new' }

test('port 或 token 变化才算身份变更', () => {
  assert.equal(runtimeIdentityChanged(previous, previous), false)
  assert.equal(runtimeIdentityChanged(previous, { ...previous, token: 'old' }), false)
  assert.equal(runtimeIdentityChanged(previous, next), true)
  assert.equal(runtimeIdentityChanged(previous, { port: 43117, token: 'new' }), true)
  assert.equal(runtimeIdentityChanged(null, next), true)
  assert.equal(runtimeIdentityChanged(previous, null), true)
})

test('kill 后读到新 runtime 时重装适配层（reclaimed）', async () => {
  const installed: BootstrapRuntime[] = []
  let calls = 0
  const result = await reclaimBackend({
    previous,
    loadRuntime: async () => {
      calls += 1
      return calls < 2 ? null : next
    },
    install: (runtime) => installed.push(runtime),
    retry: { attempts: 5, delayMs: 0 }
  })
  assert.equal(result, 'reclaimed')
  assert.deepEqual(installed, [next])
})

test('runtime 身份未变时不重装（unchanged）', async () => {
  const result = await reclaimBackend({
    previous,
    loadRuntime: async () => previous,
    install: () => assert.fail('身份未变时不应 install'),
    retry: { attempts: 2, delayMs: 0 }
  })
  assert.equal(result, 'unchanged')
})

test('自愈窗口内读不到 runtime 不立刻升硬错误语义（unavailable 留给调用方）', async () => {
  let calls = 0
  const result = await reclaimBackend({
    previous,
    loadRuntime: async () => {
      calls += 1
      return null
    },
    install: () => assert.fail('读不到时不该 install'),
    retry: { attempts: 3, delayMs: 0 }
  })
  assert.equal(result, 'unavailable')
  assert.equal(calls, 3)
})
