import { render, screen } from '@testing-library/react'
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

it('采到但未分析时展示 analysis_blocker 原因', () => {
  render(
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
  render(<RecordingStatsCard stats={{ ...base, analysis_blocker: null }} />)
  expect(screen.queryByTestId('analysis-blocker')).toBeNull()
})
