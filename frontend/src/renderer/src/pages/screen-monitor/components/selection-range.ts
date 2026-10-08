// 时间轴拖选：把「按下的那条 / 划过的这条」折成一段时间范围。
//
// 时间线是**倒序**（新的在上），下标大小与时间先后相反，所以这里统一按时间取
// 最早/最晚，调用方不必自己记方向；越界的下标夹回范围内，空列表返回 null。

import dayjs from 'dayjs'

export interface SelectableInterval {
  start_time: string
  end_time: string
}

export interface SelectionRange {
  from: string
  to: string
}

export function selectionRange<T extends SelectableInterval>(items: T[], a: number, b: number): SelectionRange | null {
  if (items.length === 0) return null
  if (a < 0 || b < 0) return null

  const lo = Math.max(0, Math.min(a, b))
  const hi = Math.min(items.length - 1, Math.max(a, b))
  if (lo > hi) return null

  const picked = items.slice(lo, hi + 1)
  const starts = picked.map((item) => dayjs(item.start_time).valueOf())
  const ends = picked.map((item) => dayjs(item.end_time).valueOf())
  if (starts.some(Number.isNaN) || ends.some(Number.isNaN)) return null

  return {
    from: dayjs(Math.min(...starts)).toISOString(),
    to: dayjs(Math.max(...ends)).toISOString()
  }
}
