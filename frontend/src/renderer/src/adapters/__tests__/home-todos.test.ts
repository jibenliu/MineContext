// 首页待办删除：两个来源的分支与顺序（不渲染界面也能测）。
import assert from 'node:assert/strict'
import { test } from 'node:test'

import { deleteHomeTodo } from '../home-todos.ts'

function deps() {
  const calls: string[] = []
  return {
    calls,
    deleteRemote: async (id: number) => {
      calls.push(`remote:${id}`)
    },
    deleteLocal: async (id: number) => {
      calls.push(`local:${id}`)
    }
  }
}

test('负数 id（本地初始数据）只删本地，不碰服务端', async () => {
  const d = deps()
  await deleteHomeTodo(-3, d)
  assert.deepEqual(d.calls, ['local:-3'], '教程条目调服务端必然失败，不能调用它')
})

test('真实条目：先服务端、后本地（顺序即契约）', async () => {
  const d = deps()
  await deleteHomeTodo(42, d)
  assert.deepEqual(d.calls, ['remote:42', 'local:42'])
})

test('服务端失败时不删本地，并把错误抛出去', async () => {
  const calls: string[] = []
  await assert.rejects(
    () =>
      deleteHomeTodo(42, {
        deleteRemote: async () => {
          calls.push('remote:42')
          throw new Error('库不可用')
        },
        deleteLocal: async () => {
          calls.push('local:42')
        }
      }),
    /库不可用/
  )
  assert.deepEqual(calls, ['remote:42'], '失败时不能出现「界面没了、库里还在」')
})
