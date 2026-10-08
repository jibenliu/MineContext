// 热力图数据的响应形状测试。
//
// 首页的热力图直接读 `heatmap:get-data` 的返回：**每一天一行、含零值**，
// 字段名固定（`todos` / `conversations` / `vaults` / `screenshots` /
// `documents` / `contexts`），前端按日期做格子。缺行会出现空洞而不是空格子，
// 字段改名则整块图变空 —— 两种都不会抛错，只会「看起来没数据」。
//
// 这里钉两件事：① 渠道映射到兼容路径；② 返回行必须带齐那些字段（含 0）。

import { calledChannels, installFakeBackend } from '@renderer/test/page-setup'
import { render, screen, waitFor } from '@testing-library/react'
import { useEffect, useState } from 'react'
import { describe, expect, it } from 'vitest'

/** daemon 的真实响应（`routes/home.rs::heatmap`）。 */
const day = {
  date: '2026-09-30',
  todos: 2,
  conversations: 1,
  vaults: 1,
  screenshots: 12,
  documents: 0,
  contexts: 3,
  total: 19
}

const HeatmapProbe: React.FC = () => {
  const [days, setDays] = useState<(typeof day)[] | null>(null)

  useEffect(() => {
    void (async () => {
      const rows = (await window.dbAPI.getHeatmapData(0, 1)) as (typeof day)[]
      setDays(rows)
    })()
  }, [])

  if (days === null) return <div>加载中…</div>

  return (
    <ul>
      {days.map((row) => (
        <li key={row.date} data-testid="heatmap-day">
          {row.date}:{row.total}
        </li>
      ))}
    </ul>
  )
}

describe('首页热力图数据（rust 后端）', () => {
  it('每天一行、字段齐全（5.43）', async () => {
    const backend = installFakeBackend({ 'heatmap:get-data': [day] }, { strict: false })

    render(<HeatmapProbe />)

    expect(await screen.findByTestId('heatmap-day')).toHaveTextContent('2026-09-30:19')

    // 形状断言写在数据上，而不是只看渲染结果：字段改名时渲染仍「成功」，
    // 但格子会全空 —— 那种绿最危险
    const rows = (await window.dbAPI.getHeatmapData(0, 1)) as Record<string, unknown>[]
    for (const field of ['date', 'todos', 'conversations', 'vaults', 'screenshots', 'documents', 'contexts', 'total']) {
      expect(rows[0]).toHaveProperty(field)
    }

    await waitFor(() => expect(calledChannels(backend)).toContain('heatmap:get-data'))
  })
})
