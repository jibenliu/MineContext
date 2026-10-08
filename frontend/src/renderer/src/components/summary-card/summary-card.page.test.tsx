// 页面级测试：总结卡片。
//
// 后端 `/api/v1/summaries` 早就有了，前端此前既没有渠道映射也没有卡片。
// 这一组测试同时钉住三件事：
//   1. 卡片真的通过适配层取数（`v1:summaries`）；
//   2. 字段渲染完整（范围 / 标题 / 正文 / 证据计数）；
//   3. **`quality=fallback` 必须有可见且文案不同的徽标** ——
//      兜底总结和模型总结看起来一样，用户就分不清「这是模型写的还是模板套的」。

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import { calledChannels, installFakeBackend } from '@renderer/test/page-setup'
import { render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { SummaryCard } from './summary-card'

// mock 的**形状**从契约 fixture 取（那份 fixture 由 daemon 侧测试校验过
// 「形状 == 真实返回」）。手写 mock 正是白屏的根因：后端返回 `{summaries: [...]}`，
// 而 mock 写成了裸数组 —— 测试绿、真实白。改形状就会在这里红。
// vitest 里 `import.meta.url` 是 http 的，取文件系统路径要用运行目录（= frontend/）
const fixturePath = resolve(process.cwd(), '../fixtures/contract/response-shapes.json')
const shapes = JSON.parse(readFileSync(fixturePath, 'utf8')) as {
  channels: Record<string, { sample: { summaries: Record<string, unknown>[] } }>
}

const sampleSummary = shapes.channels['v1:summaries'].sample.summaries[0]

const modelSummary = {
  ...sampleSummary,
  id: 'sum-1',
  stage_id: 'stage-1',
  kind: 'stage',
  title: '上午：写导入脚本',
  body_markdown: '把旧库的笔记与待办导进新版数据目录。',
  quality: 'model',
  start: '2026-09-30T09:00:00+08:00',
  end: '2026-09-30T11:30:00+08:00',
  evidence_count: 7
}

const fallbackSummary = {
  ...modelSummary,
  id: 'sum-2',
  quality: 'fallback',
  title: '下午：修复导出',
  body_markdown: '（模板兜底）2 个活动，持续 45 分钟。'
}

describe('summary-card（rust 后端）', () => {
  it('渲染总结的关键字段（4.51）', async () => {
    const backend = installFakeBackend({ 'v1:summaries': { summaries: [modelSummary] } }, { strict: false })

    render(<SummaryCard />)

    expect(await screen.findByText('上午：写导入脚本')).toBeInTheDocument()
    expect(screen.getByText(/把旧库的笔记与待办导进新版数据目录/)).toBeInTheDocument()
    // 范围与证据计数：用户判断「这条总结覆盖了多久、基于多少条证据」
    expect(screen.getByText(/09:00/)).toBeInTheDocument()
    expect(screen.getByText(/11:30/)).toBeInTheDocument()
    expect(screen.getByText(/7/)).toBeInTheDocument()

    // 真的通过适配层取数 —— 否则「渲染出来了」可能只是写死的占位
    await waitFor(() => expect(calledChannels(backend)).toContain('v1:summaries'))
  })

  it('fallback 质量标记必须可见且与 model 不同（4.52）', async () => {
    installFakeBackend({ 'v1:summaries': { summaries: [fallbackSummary] } }, { strict: false })

    render(<SummaryCard />)

    const badge = await screen.findByTestId('summary-quality')
    expect(badge).toHaveTextContent(/兜底|fallback/i)
    expect(badge.className).toMatch(/fallback/)
    // 「可见」不能只靠类名：兜底徽标必须有真正区分颜色的样式
    expect(badge.className).toContain('bg-[var(--color-warning-light-1)]')
  })

  it('model 质量标记不显示成兜底', async () => {
    installFakeBackend({ 'v1:summaries': { summaries: [modelSummary] } }, { strict: false })

    render(<SummaryCard />)

    const badge = await screen.findByTestId('summary-quality')
    expect(badge.className).toMatch(/model/)
    expect(badge.className).toContain('bg-[var(--color-success-light-1)]')
    expect(badge).not.toHaveTextContent(/兜底/)
  })

  it('没有总结时显示空态而不是空白', async () => {
    installFakeBackend({ 'v1:summaries': { summaries: [] } }, { strict: false })

    render(<SummaryCard />)

    expect(await screen.findByText(/还没有总结/)).toBeInTheDocument()
  })
})
