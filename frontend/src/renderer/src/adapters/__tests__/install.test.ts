// 适配层安装后，业务代码依赖的全局形状必须齐全。

import assert from 'node:assert/strict'
import { test } from 'node:test'

import { getLogger, setLogSink } from '../../../../../packages/shared/logger/renderer.ts'
import { installAdapters } from '../install.ts'
import type { Backend } from '../types.ts'

function noopBackend(): Backend {
  return {
    kind: 'http',
    async invoke<T>(): Promise<T> {
      return undefined as T
    },
    subscribe() {
      return () => {}
    }
  }
}

test('install_exposes_all_legacy_globals', () => {
  const target: Record<string, unknown> = {}
  installAdapters({ backend: noopBackend(), target })

  for (const name of ['dbAPI', 'screenMonitorAPI', 'fileService', 'serverPushAPI', 'eventLoop', 'api', 'electron']) {
    assert.ok(target[name] !== undefined, `缺少全局 ${name}`)
  }

  const electron = target.electron as { ipcRenderer?: unknown }
  assert.ok(electron.ipcRenderer, '缺少 window.electron.ipcRenderer shim')

  const db = target.dbAPI as Record<string, unknown>
  assert.equal(typeof db.getAllVaults, 'function')
  assert.equal(typeof db.addTask, 'function')

  const monitor = target.screenMonitorAPI as Record<string, unknown>
  assert.equal(typeof monitor.getRecordingStats, 'function')
})

// [已移除] resolveBackendKind：应用只有 daemon 一条后端路径

// [已移除] resolveBackendKind：应用只有 daemon 一条后端路径

// 渲染层日志落盘：有外壳就转发给外壳命令，没有就把出口卸掉
test('install_wires_renderer_log_to_shell_command', async () => {
  const calls: { command: string; payload?: Record<string, unknown> }[] = []
  const target: Record<string, unknown> = {}

  installAdapters({
    backend: noopBackend(),
    target,
    shellCapabilities: {
      invoke: async (command, payload) => {
        calls.push({ command, payload })
        return undefined
      }
    }
  })

  getLogger('Wiring').info('hello', { a: 1 })
  // 转发是 fire-and-forget（微任务），给它一个 tick
  await new Promise((resolve) => setTimeout(resolve, 0))
  setLogSink(undefined)

  assert.deepEqual(calls, [
    { command: 'renderer_log', payload: { level: 'info', message: '[renderer - Wiring] hello {"a":1}' } }
  ])
})

test('install_without_shell_detaches_the_log_sink', () => {
  const received: string[] = []
  setLogSink((_level, message) => received.push(message))
  const target: Record<string, unknown> = {}

  // 没有外壳能力时必须把出口卸掉：否则日志会被送进一个不存在的命令
  installAdapters({ backend: noopBackend(), target })
  getLogger('Wiring').info('should not be forwarded')
  setLogSink(undefined)

  assert.deepEqual(received, [])
})
