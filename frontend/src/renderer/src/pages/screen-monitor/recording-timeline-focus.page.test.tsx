// 助手引用落到时间线：目标活动高亮（截图挂在该条目下）。

import { installFakeBackend } from '@renderer/test/page-setup'
import { render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import { RecordingTimeline } from './components/recording-timeline'

vi.mock('./hooks/use-activity-provenance', () => ({
  useActivityProvenance: () => ({
    badgeFor: () => null,
    rename: async () => undefined,
    merge: async () => undefined,
    split: async () => undefined
  })
}))

describe('时间线：引用聚焦', () => {
  it('focusActivityId 给对应条目高亮标记', () => {
    installFakeBackend({}, { strict: false })
    render(
      <RecordingTimeline
        isMonitoring={false}
        isToday={false}
        canRecord={false}
        recordingStats={null}
        focusActivityId="act-7"
        activities={[
          {
            id: 'act-7',
            title: '导入工作',
            content: '',
            start_time: '2026-10-08 10:00:00',
            end_time: '2026-10-08 10:30:00',
            resources: [{ type: 'image', id: 'img-1', path: '/tmp/a.png' }]
          }
        ]}
      />
    )
    const row = screen.getByTestId('timeline-item-0')
    expect(row).toHaveAttribute('data-activity-id', 'act-7')
    expect(row.className).toMatch(/primary-light/)
  })
})
