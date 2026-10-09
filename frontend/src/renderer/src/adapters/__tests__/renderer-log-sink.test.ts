import assert from 'node:assert/strict'
import { test } from 'node:test'

import { getLogger, setLogSink } from '../../../../../packages/shared/logger/renderer.ts'
import { installRendererLogSink } from '../renderer-log-sink.ts'

test('有 __TAURI__.core.invoke 时，bootstrap 前即可把日志转到 renderer_log', async () => {
  const calls: { command: string; payload?: Record<string, unknown> }[] = []
  const target = {
    __TAURI__: {
      core: {
        invoke: async (command: string, payload?: Record<string, unknown>) => {
          calls.push({ command, payload })
          return undefined
        }
      }
    }
  }

  assert.equal(installRendererLogSink(target), true)
  getLogger('EarlySink').warn('boot before http install')
  await new Promise((resolve) => setTimeout(resolve, 0))
  setLogSink(undefined)

  assert.equal(calls.length, 1)
  assert.equal(calls[0]?.command, 'renderer_log')
  assert.match(String(calls[0]?.payload?.message), /boot before http install/)
})

test('没有外壳时卸掉 sink，避免把日志丢进不存在的命令', () => {
  const received: string[] = []
  setLogSink((_level, message) => received.push(message))
  assert.equal(installRendererLogSink({}), false)
  getLogger('EarlySink').info('should not forward')
  setLogSink(undefined)
  assert.deepEqual(received, [])
})
