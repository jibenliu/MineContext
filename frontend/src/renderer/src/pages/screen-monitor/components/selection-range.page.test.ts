// 页面级测试（无需 DOM）：拖选下标 → 时间范围。

import dayjs from 'dayjs'
import { describe, expect, it } from 'vitest'

import { selectionRange } from './selection-range'

// 时间线是倒序：下标 0 最新
const items = [
  { start_time: '2026-10-04 10:00:00', end_time: '2026-10-04 10:30:00' },
  { start_time: '2026-10-04 09:00:00', end_time: '2026-10-04 09:30:00' },
  { start_time: '2026-10-04 08:00:00', end_time: '2026-10-04 08:30:00' }
]

const iso = (value: string) => dayjs(value).toISOString()

describe('screen-monitor/selection-range', () => {
  it('跨多条选择时取最早开始与最晚结束', () => {
    expect(selectionRange(items, 0, 2)).toEqual({
      from: iso('2026-10-04 08:00:00'),
      to: iso('2026-10-04 10:30:00')
    })
  })

  it('反向拖（从下往上）得到同一段范围', () => {
    expect(selectionRange(items, 2, 0)).toEqual(selectionRange(items, 0, 2))
  })

  it('只选一条时就是那一条的起止，不扩成整天', () => {
    const range = selectionRange(items, 1, 1)
    expect(range).toEqual({ from: iso('2026-10-04 09:00:00'), to: iso('2026-10-04 09:30:00') })
    expect(dayjs(range?.to).diff(dayjs(range?.from), 'minute')).toBe(30)
  })

  it('越界下标夹回范围，空列表与负数下标返回 null', () => {
    expect(selectionRange(items, 0, 99)).toEqual(selectionRange(items, 0, 2))
    expect(selectionRange(items, 0, -1)).toBeNull()
    expect(selectionRange([], 0, 0)).toBeNull()
  })
})
