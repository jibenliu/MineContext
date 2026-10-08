// 页面级测试（无需 DOM）：对话里的总结指令 → 范围。
//
// 「识别不出就返回 null」和「不把闲聊当指令」是重点：调用方据此明确回复用户，
// 而不是猜一个范围去生成总结。

import dayjs from 'dayjs'
import { describe, expect, it } from 'vitest'

import { parseSummaryIntent } from './summary-intent'

const now = dayjs('2026-10-04T10:00:00')

describe('ai-assistant/summary-intent', () => {
  it('今天 / 昨天 / 前天', () => {
    expect(parseSummaryIntent('总结今天', now)).toMatchObject({ from: '2026-10-04', to: '2026-10-04' })
    expect(parseSummaryIntent('帮我总结一下昨天', now)).toMatchObject({ from: '2026-10-03', to: '2026-10-03' })
    expect(parseSummaryIntent('前天的汇总', now)).toMatchObject({ from: '2026-10-02', to: '2026-10-02' })
  })

  it('最近 N 天含今天，且天数按当天算', () => {
    expect(parseSummaryIntent('总结最近 7 天', now)).toMatchObject({ from: '2026-09-28', to: '2026-10-04' })
    expect(parseSummaryIntent('最近3日小结', now)).toMatchObject({ from: '2026-10-02', to: '2026-10-04' })
    expect(parseSummaryIntent('总结最近一周', now)).toMatchObject({ from: '2026-09-28', to: '2026-10-04' })
  })

  it('显式日期区间，反着说也按时间先后摆正', () => {
    expect(parseSummaryIntent('总结 2026-09-01 到 2026-09-03', now)).toMatchObject({
      from: '2026-09-01',
      to: '2026-09-03'
    })
    expect(parseSummaryIntent('总结 2026-09-03 到 2026-09-01', now)).toMatchObject({
      from: '2026-09-01',
      to: '2026-09-03'
    })
  })

  it('不是指令就不接管：闲聊与无时段的请求都返回 null', () => {
    expect(parseSummaryIntent('今天天气怎么样', now)).toBeNull()
    expect(parseSummaryIntent('帮我总结一下', now)).toBeNull()
    expect(parseSummaryIntent('', now)).toBeNull()
    expect(parseSummaryIntent('总结最近 0 天', now)).toBeNull()
  })

  it('范围给的是纯日期，交给后端按配置时区展开（前端不改语义）', () => {
    const intent = parseSummaryIntent('总结昨天', now)
    expect(intent?.from).toMatch(/^\d{4}-\d{2}-\d{2}$/)
    expect(intent?.to).toMatch(/^\d{4}-\d{2}-\d{2}$/)
    expect(intent?.label).toBe('昨天')
  })
})
