import {
  confirmHandoff,
  dismissRisk,
  exportHandoff,
  getFollowUps,
  getLearningTopics,
  getReviewPlan,
  getRiskReport,
  getSalesTimeline,
  getStuckPatterns,
  getVisitPrep,
  listHandoffCandidates,
  listRisks
} from '@renderer/services/insights'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, expect, it, vi } from 'vitest'

import { InsightsPage } from './insights-page'

vi.mock('@renderer/services/insights', () => ({
  listRisks: vi.fn(),
  getRiskReport: vi.fn(),
  dismissRisk: vi.fn(),
  getSalesTimeline: vi.fn(),
  getFollowUps: vi.fn(),
  getVisitPrep: vi.fn(),
  getLearningTopics: vi.fn(),
  getStuckPatterns: vi.fn(),
  getReviewPlan: vi.fn(),
  listHandoffCandidates: vi.fn(),
  confirmHandoff: vi.fn(),
  exportHandoff: vi.fn()
}))

vi.mock('@arco-design/web-react', async () => {
  const actual = await vi.importActual<typeof import('@arco-design/web-react')>('@arco-design/web-react')
  return {
    ...actual,
    Message: { ...actual.Message, success: vi.fn(), error: vi.fn() }
  }
})

beforeEach(() => {
  vi.mocked(listRisks).mockResolvedValue([
    {
      id: 'r1',
      kind: 'commitment',
      text: 'Need to finish by Friday',
      source_id: 'a1',
      source_kind: 'activity',
      at: 1,
      score: 80
    }
  ])
  vi.mocked(getRiskReport).mockResolvedValue('# 项目风险与遗漏\n\n## 承诺\n\n- Need to finish by Friday\n')
  vi.mocked(dismissRisk).mockResolvedValue(undefined)
  vi.mocked(getSalesTimeline).mockResolvedValue([
    {
      contact_id: 'contoso',
      display_name: 'Contoso',
      summary: 'Meeting with Contoso',
      source_id: 's1',
      at: 1
    }
  ])
  vi.mocked(getFollowUps).mockResolvedValue([
    { contact_id: 'contoso', hint: '跟进承诺：报价', reason: 'open_commitment', at: 1 }
  ])
  vi.mocked(getVisitPrep).mockResolvedValue({
    contact_id: 'contoso',
    display_name: 'Contoso',
    prep_notes: '拜访准备：Contoso\n',
    recent: [],
    commitments: [],
    follow_ups: []
  })
  vi.mocked(getLearningTopics).mockResolvedValue([
    { topic_id: 'rust', label: 'Rust', hit_count: 2, last_at: 1, source_ids: ['l1'] }
  ])
  vi.mocked(getStuckPatterns).mockResolvedValue([
    {
      pattern_id: 'error-generic',
      label: '编译/运行错误',
      repeats: 2,
      last_at: 1,
      hint: '反复遇到错误'
    }
  ])
  vi.mocked(getReviewPlan).mockResolvedValue({
    generated_at: 1,
    summary: '共 3 个复习项',
    items: [{ topic_id: 'rust', label: 'Rust', due_at: 2, interval_days: 1, reason: 'hit_count=2' }]
  })
  vi.mocked(listHandoffCandidates).mockResolvedValue([
    {
      id: 'cand-h1',
      kind: 'incident',
      title: '事故：超时',
      body: '事故：超时回滚',
      source_id: 'h1',
      at: 1,
      confirmed: false
    }
  ])
  vi.mocked(confirmHandoff).mockResolvedValue(undefined)
  vi.mocked(exportHandoff).mockResolvedValue({
    manifest: {
      format: 'minecontext-handoff-pack',
      schema_version: 1,
      exported_at_ms: 1,
      item_count: 0
    },
    items: [],
    markdown: '# 交接包（本地）\n\n（空包）\n'
  })
})

it('lists risks, shows report, and dismisses a false positive', async () => {
  render(<InsightsPage />)
  await screen.findByTestId('insights-page')
  await screen.findByTestId('insights-risk-report')
  expect(screen.getByText('Need to finish by Friday')).toBeTruthy()

  vi.mocked(listRisks).mockResolvedValueOnce([])
  fireEvent.click(screen.getByTestId('risk-dismiss-r1'))
  await waitFor(() => {
    expect(dismissRisk).toHaveBeenCalledWith('r1')
  })
})

it('shows sales visit prep and learning review plan', async () => {
  render(<InsightsPage />)
  await screen.findByTestId('insights-page')
  // Tabs keep panels mounted (lazyload=false); content should be present
  await waitFor(() => {
    expect(screen.getByTestId('insights-visit-prep').textContent).toContain('拜访准备')
  })
  expect(screen.getByTestId('insights-review-summary').textContent).toContain('复习')
})

it('confirms handoff candidate and exports local pack', async () => {
  render(<InsightsPage />)
  await screen.findByTestId('insights-page')
  fireEvent.click(await screen.findByTestId('handoff-confirm-cand-h1'))
  await waitFor(() => {
    expect(confirmHandoff).toHaveBeenCalledWith('cand-h1')
  })
  fireEvent.click(screen.getByTestId('handoff-export'))
  await waitFor(() => {
    expect(exportHandoff).toHaveBeenCalled()
  })
  expect(screen.getByTestId('insights-handoff-markdown').textContent).toContain('交接包')
})
