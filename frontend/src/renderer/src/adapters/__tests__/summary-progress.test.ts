// 后台作业完成的判定与通知：纯函数 + 一次订阅，因此不需要 DOM。
import assert from 'node:assert/strict'
import { test } from 'node:test'

import { completedJobId, watchSummaryProgress } from '../summary-progress.ts'

test('只有「分块全部完成」才算终态', () => {
  assert.equal(completedJobId({ job_id: 'job-1', chunks_done: 3, chunks_total: 3 }), 'job-1')
  assert.equal(completedJobId({ job_id: 'job-1', chunks_done: 2, chunks_total: 3 }), undefined)
  assert.equal(completedJobId({ job_id: 'job-1', chunks_done: 0, chunks_total: 0 }), undefined)
  assert.equal(completedJobId({ chunks_done: 3, chunks_total: 3 }), undefined, '缺 job_id 不通知')
  assert.equal(completedJobId('running'), undefined)
  assert.equal(completedJobId(null), undefined)
})

test('完成时通知一次，重复帧不重复弹', async () => {
  const handlers: Array<(frame: unknown) => void> = []
  const sent: Array<{ title: string; message: string }> = []
  const done: string[] = []

  const stop = watchSummaryProgress(
    (handler) => {
      handlers.push(handler)
      return () => handlers.splice(0, handlers.length)
    },
    async (notification) => {
      sent.push(notification)
    },
    (jobId) => done.push(jobId)
  )

  handlers[0]({ job_id: 'job-7', chunks_done: 1, chunks_total: 2 })
  handlers[0]({ job_id: 'job-7', chunks_done: 2, chunks_total: 2 })
  handlers[0]({ job_id: 'job-7', chunks_done: 2, chunks_total: 2 })
  await Promise.resolve()

  assert.deepEqual(done, ['job-7'], '完成回调只触发一次')
  assert.equal(sent.length, 1, '同一次作业只弹一条通知')
  assert.match(sent[0].title, /总结/)
  stop()
})

test('外壳没有通知能力时不抛错，只记录', async () => {
  const logs: string[] = []
  const stop = watchSummaryProgress(
    (handler) => {
      handler({ job_id: 'job-9', chunks_done: 1, chunks_total: 1 })
      return () => {}
    },
    undefined,
    () => {},
    (message) => logs.push(message)
  )
  await Promise.resolve()
  assert.equal(logs.length, 1)
  assert.match(logs[0], /没有通知能力/)
  stop()
})

test('通知失败不影响完成回调（总结已经生成是事实）', async () => {
  const done: string[] = []
  const logs: string[] = []
  const stop = watchSummaryProgress(
    (handler) => {
      handler({ job_id: 'job-5', chunks_done: 2, chunks_total: 2 })
      return () => {}
    },
    async () => {
      throw new Error('外壳未声明 notification 能力')
    },
    (jobId) => done.push(jobId),
    (message) => logs.push(message)
  )
  await Promise.resolve()
  await Promise.resolve()
  assert.deepEqual(done, ['job-5'])
  assert.equal(logs.length, 1)
  assert.match(logs[0], /通知发送失败/)
  stop()
})
