// 范围折算规则：纯日期折成整天，带时间的范围**必须原样保留**。
//
// 拖选给的是分钟级范围；如果这里折成整天，用户选了 30 分钟却会拿到一整天的
// 总结，而且界面上没有任何提示 —— 这是会说谎的行为，用测试钉住。

import dayjs from 'dayjs'
import { describe, expect, it } from 'vitest'

import { dayRange, hasTimePrecision } from './adhoc-summary'

describe('adhoc/day-range', () => {
  it('纯日期折成当天开头与结尾（与搜索页同一套口径）', () => {
    const [start, end] = dayRange('2026-10-04', '2026-10-04')

    expect(dayjs(start).format('HH:mm:ss')).toBe('00:00:00')
    expect(dayjs(end).format('HH:mm:ss')).toBe('23:59:59')
    expect(dayjs(start).valueOf()).toBeLessThan(dayjs(end).valueOf())
  })

  it('带时间的范围原样保留：80 分钟就是 80 分钟，不扩成一天', () => {
    const [start, end] = dayRange('2026-10-04T14:20:00+08:00', '2026-10-04T15:40:00+08:00')

    expect(dayjs(end).diff(dayjs(start), 'minute')).toBe(80)
  })

  it('识别哪些写法带时间', () => {
    expect(hasTimePrecision('2026-10-04')).toBe(false)
    expect(hasTimePrecision('2026-10-04T14:20:00+08:00')).toBe(true)
    expect(hasTimePrecision('2026-10-04 14:20:00')).toBe(true)
  })
})
