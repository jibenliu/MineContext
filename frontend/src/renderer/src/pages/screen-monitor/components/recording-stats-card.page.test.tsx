import { render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { expect, it } from 'vitest'

import RecordingStatsCard, { type RecordingStats } from './recording-stats-card'

const base: RecordingStats = {
  captured_screenshots: 40,
  processed_screenshots: 0,
  failed_screenshots: 0,
  pending_analyses: 40,
  generated_activities: 0,
  next_activity_eta_seconds: 0,
  recent_errors: [],
  recent_screenshots: []
}

function renderCard(ui: React.ReactElement) {
  return render(<MemoryRouter>{ui}</MemoryRouter>)
}

it('采到但未分析时展示 analysis_blocker 原因', () => {
  renderCard(
    <RecordingStatsCard
      stats={{
        ...base,
        analysis_blocker: {
          code: 'analyses_pending_unwired',
          message: '有 40 条截图分析仍为 pending：截图视觉分析作业尚未接线'
        }
      }}
    />
  )
  const blocker = screen.getByTestId('analysis-blocker')
  expect(blocker).toHaveTextContent(/pending/)
  expect(blocker).toHaveTextContent(/40/)
})

it('没有 blocker 时不渲染原因行', () => {
  renderCard(<RecordingStatsCard stats={{ ...base, analysis_blocker: null }} />)
  expect(screen.queryByTestId('analysis-blocker')).toBeNull()
})

it('带 action 时展示指向设置 section 的入口', () => {
  renderCard(
    <RecordingStatsCard
      stats={{
        ...base,
        analysis_blocker: {
          code: 'ai_upload_disabled',
          message: '尚未允许 AI 出网',
          action: { target: 'settings_ai_upload', label: '去设置开启' }
        }
      }}
    />
  )
  const action = screen.getByTestId('analysis-blocker-action')
  expect(action).toHaveTextContent('去设置开启')
  expect(action).toHaveAttribute('href', '/settings?section=ai-upload')
})

it('indexing_pause 时展示暂停原因与恢复入口', () => {
  renderCard(
    <RecordingStatsCard
      stats={{
        ...base,
        processed_screenshots: 3,
        indexing_pause: {
          paused: true,
          code: 'api_key_invalid',
          message: '向量索引已暂停：API Key 无效或已过期。',
          action: { target: 'resume_indexing', label: '恢复索引' }
        }
      }}
    />
  )
  expect(screen.getByTestId('indexing-pause')).toHaveTextContent(/向量索引已暂停/)
  expect(screen.getByTestId('indexing-pause-action')).toHaveTextContent('恢复索引')
})
