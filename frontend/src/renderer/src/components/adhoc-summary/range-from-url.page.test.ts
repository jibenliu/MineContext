// 页面级测试（无需 DOM）：路由参数 → 总结范围的取值规则。

import { describe, expect, it } from 'vitest'

import { rangeFromHash } from './range-from-url'

const fallback = { from: '2026-10-01', to: '2026-10-02' }

describe('adhoc/range-from-url', () => {
  it('没有查询串时用默认范围', () => {
    expect(rangeFromHash('#/summaries', fallback)).toEqual(fallback)
    expect(rangeFromHash('', fallback)).toEqual(fallback)
  })

  it('取出时间戳精度的范围（拖选给的是分钟级）', () => {
    const hash = '#/summaries?from=2026-10-04T14%3A20%3A00%2B08%3A00&to=2026-10-04T15%3A40%3A00%2B08%3A00'
    expect(rangeFromHash(hash, fallback)).toEqual({
      from: '2026-10-04T14:20:00+08:00',
      to: '2026-10-04T15:40:00+08:00'
    })
  })

  it('只给一半时不猜另一半，退回默认范围', () => {
    expect(rangeFromHash('#/summaries?from=2026-10-04', fallback)).toEqual(fallback)
    expect(rangeFromHash('#/summaries?to=2026-10-04', fallback)).toEqual(fallback)
    expect(rangeFromHash('#/summaries?from=&to=', fallback)).toEqual(fallback)
  })
})
