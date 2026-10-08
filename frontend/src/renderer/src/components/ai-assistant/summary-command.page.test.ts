// 页面级测试（无需 DOM）：总结指令的分流判断。

import dayjs from 'dayjs'
import { describe, expect, it, vi } from 'vitest'

import { dispatchedMessage, dispatchSummaryCommand } from './summary-command'

const now = dayjs('2026-10-04T10:00:00')

function deps(submit = vi.fn().mockResolvedValue({ job_id: 'job-1' })) {
  return {
    submit,
    onDispatched: vi.fn(),
    onFailed: vi.fn()
  }
}

describe('ai-assistant/summary-command', () => {
  it('命中指令：提交作业、给出提示，并告诉调用方「别发给模型」', async () => {
    const d = deps()

    const handled = await dispatchSummaryCommand('总结昨天', d, now)

    expect(handled).toBe(true)
    expect(d.submit).toHaveBeenCalledWith('2026-10-03', '2026-10-03')
    expect(d.onDispatched).toHaveBeenCalledWith('昨天', '/summaries?job_id=job-1&from=2026-10-03&to=2026-10-03')
    expect(d.onFailed).not.toHaveBeenCalled()
  })

  it('不是指令：返回 false 且什么都不做，按普通对话继续', async () => {
    const d = deps()

    expect(await dispatchSummaryCommand('帮我看看这段代码', d, now)).toBe(false)
    expect(await dispatchSummaryCommand('今天天气怎么样', d, now)).toBe(false)
    expect(d.submit).not.toHaveBeenCalled()
    expect(d.onDispatched).not.toHaveBeenCalled()
  })

  it('提交失败也算已处理：明说失败，而不是把这句话回退成普通提问', async () => {
    const d = deps(vi.fn().mockRejectedValue(new Error('daemon 未就绪')))

    const handled = await dispatchSummaryCommand('总结今天', d, now)

    expect(handled).toBe(true)
    expect(d.onFailed).toHaveBeenCalledWith(expect.stringContaining('生成失败'))
    expect(d.onDispatched).not.toHaveBeenCalled()
  })

  it('缺失作业 id 不宣称提交成功', async () => {
    const dependencies = deps(vi.fn().mockResolvedValue({}))
    expect(await dispatchSummaryCommand('总结今天', dependencies, now)).toBe(true)
    expect(dependencies.onFailed).toHaveBeenCalled()
    expect(dependencies.onDispatched).not.toHaveBeenCalled()
  })
})

// 确认文案要说清进度在哪：作业在后台跑，界面若不提，用户会以为没开始。
it('确认文案指明进度位置', () => {
  const message = dispatchedMessage('昨天')

  expect(message).toContain('昨天')
  expect(message).toContain('总结页')
})
