// 从路由查询串里取任意时段总结的范围：时间轴拖选后深链到总结页用这个。
//
// 范围允许是时间戳（拖选给的是分钟级精度），也可以是纯日期（预设按钮）。
// 只给一半时不猜另一半，宁可退回默认范围，也不生成一个用户没选过的区间。

export interface AdhocRangeStrings {
  from: string
  to: string
}

export function rangeFromHash(hash: string, fallback: AdhocRangeStrings): AdhocRangeStrings {
  const queryAt = hash.indexOf('?')
  if (queryAt < 0) return fallback

  const params = new URLSearchParams(hash.slice(queryAt + 1))
  const from = params.get('from')?.trim()
  const to = params.get('to')?.trim()
  if (!from || !to) return fallback
  return { from, to }
}
