// `window.dbAPI` 的形状必须与 preload 完全一致。
//
// 现有业务代码（use-home-info.ts / use-vault.ts / vault-thunk.ts）直接依赖
// 这些渠道名与返回形状。适配层只要有一处不一致，对应功能就会坏。

import assert from 'node:assert/strict'
import { test } from 'node:test'

import { createDbApi } from '../db-api.ts'
import type { Backend } from '../types.ts'

interface Call {
  channel: string
  args: unknown[]
}

function recordingBackend(result: unknown = undefined): Backend & { calls: Call[] } {
  const calls: Call[] = []
  return {
    kind: 'http',
    calls,
    async invoke<T>(channel: string, ...args: unknown[]): Promise<T> {
      calls.push({ channel, args })
      return result as T
    },
    subscribe() {
      return () => {}
    }
  }
}

test('db_api_uses_legacy_channel_names', async () => {
  const backend = recordingBackend()
  const db = createDbApi(backend)

  await db.getAllActivities()
  await db.getNewActivities('2026-09-30 00:00:00')
  await db.getAllVaults()
  await db.getVaultsByParentId(-1)
  await db.getVaultById(3)
  await db.getVaultByTitle('Summary')
  await db.deleteVaultById(3)
  await db.getFolders()
  await db.softDeleteVaultById(3)
  await db.restoreVaultById(3)
  await db.hardDeleteVaultById(3)
  await db.getAllTips()
  await db.getLatestActivity()
  await db.getTasks('a', 'b')
  await db.updateTask(1, { content: 'x' })
  await db.deleteTask(1)
  await db.toggleTaskStatus(1)
  await db.getHeatmapData(1, 2)

  assert.deepEqual(
    backend.calls.map((c) => c.channel),
    [
      'database:get-all-activities',
      'database:get-new-activities',
      'database:get-all-vaults',
      'database:get-vaults-by-parent-id',
      'database:get-vault-by-id',
      'database:get-vault-by-title',
      'database:delete-vault-by-id',
      'database:get-folders',
      'database:soft-delete-vault-by-id',
      'database:restore-vault-by-id',
      'database:hard-delete-vault-by-id',
      'database:get-all-tips',
      'database:get-latest-activity',
      'database:get-all-tasks',
      'database:update-task',
      'database:delete-task',
      'database:toggle-task-status',
      'heatmap:get-data'
    ]
  )
})

test('db_api_passes_arguments_through_unchanged', async () => {
  const backend = recordingBackend()
  const db = createDbApi(backend)

  await db.getVaultsByParentId(null)
  await db.updateVaultById(7, { title: '新标题' })
  await db.createFolder('Summary', 3)
  await db.getVaultsByDocumentType('DailyReport')

  assert.deepEqual(backend.calls[0], { channel: 'database:get-vaults-by-parent-id', args: [null] })
  assert.deepEqual(backend.calls[1], {
    channel: 'database:update-vault-by-id',
    args: [7, { title: '新标题' }]
  })
  assert.deepEqual(backend.calls[2], { channel: 'database:create-folder', args: ['Summary', 3] })
  assert.deepEqual(backend.calls[3], {
    channel: 'database:get-vaults-by-document-type',
    args: ['DailyReport']
  })
})

// 业务代码直接读 better-sqlite3 的 RunResult（use-home-info.ts:52），
// 所以返回对象必须原样保留 lastInsertRowid 字段名。
test('add_task_returns_last_insert_rowid_field', async () => {
  const backend = recordingBackend({ lastInsertRowid: 7, changes: 1 })
  const db = createDbApi(backend)

  const result = (await db.addTask({ content: '写测试' })) as {
    lastInsertRowid?: number
    changes?: number
  }

  assert.equal(result.lastInsertRowid, 7, '缺少 lastInsertRowid 会让首页新增待办失效')
  assert.equal(result.changes, 1)
  assert.deepEqual(backend.calls[0], {
    channel: 'database:add-task',
    args: [{ content: '写测试' }]
  })
})

test('insert_vault_returns_id_field', async () => {
  const backend = recordingBackend({ id: 11 })
  const db = createDbApi(backend)

  const result = (await db.insertVault({ title: 'x' })) as { id?: number }
  assert.equal(result.id, 11)
  assert.deepEqual(backend.calls[0], { channel: 'database:insert-vault', args: [{ title: 'x' }] })
})

test('db_api_surfaces_backend_errors', async () => {
  const failing: Backend = {
    kind: 'http',
    async invoke<T>(): Promise<T> {
      throw new Error('backend down')
    },
    subscribe() {
      return () => {}
    }
  }
  const db = createDbApi(failing)

  await assert.rejects(() => db.getAllVaults(), /backend down/)
})
