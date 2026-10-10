// 页面级测试：分组时间线 —— 按时段 / 分类折叠，而不是一堵截图墙。

import { installFakeBackend } from '@renderer/test/page-setup'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import { RecordingTimeline } from './components/recording-timeline'

vi.mock('./components/activitie-timeline-item', () => ({
  ActivityTimelineItem: ({ activity }: { activity: { title: string } }) => (
    <span data-testid="activity-title">{activity.title}</span>
  )
}))

const activity = (id: number, start: string, end: string, title: string, category: string | null, images = 1) => ({
  id: String(id),
  start_time: start,
  end_time: end,
  title,
  content: '',
  category,
  resources: Array.from({ length: images }, (_, i) => ({
    type: 'image',
    id: `${id}-${i}`,
    path: `shots/${id}-${i}.png`
  }))
})

const items = [
  activity(11, '2026-10-04 10:10:00', '2026-10-04 10:25:00', '写导入脚本', '开发', 2),
  activity(12, '2026-10-04 10:30:00', '2026-10-04 10:50:00', '修测试', '开发', 1),
  activity(13, '2026-10-04 09:05:00', '2026-10-04 09:40:00', '需求评审', '需求', 3)
]

describe('时间线：按时段与分类分组', () => {
  it('默认分组视图渲染时段与分类摘要，可切回扁平列表', async () => {
    installFakeBackend(
      {
        'v1:activities': {
          activities: items.map((item) => ({
            id: `act-${item.id}`,
            legacy_id: Number(item.id),
            start: item.start_time,
            end: item.end_time,
            title: item.title,
            original_title: item.title,
            category: item.category,
            origin: { kind: 'inferred', model: 'test' },
            confidence: 0.8,
            evidence: [],
            is_user_modified: false,
            derived_from_seq: 1
          }))
        }
      },
      { strict: false }
    )

    render(
      <RecordingTimeline
        isMonitoring={false}
        isToday={false}
        canRecord={false}
        activities={items}
        recordingStats={null}
      />
    )

    expect(screen.getByTestId('timeline-view-grouped')).toHaveAttribute('aria-pressed', 'true')
    expect(await screen.findByTestId('timeline-period-2026-10-04T10')).toBeInTheDocument()
    expect(screen.getByTestId('timeline-period-2026-10-04T09')).toBeInTheDocument()
    expect(screen.getByTestId('timeline-group-2026-10-04T10-开发')).toHaveTextContent('开发')
    expect(screen.getByTestId('timeline-group-2026-10-04T10-开发')).toHaveTextContent('3')

    fireEvent.click(screen.getByTestId('timeline-view-list'))
    await waitFor(() => {
      expect(screen.getByTestId('timeline-view-list')).toHaveAttribute('aria-pressed', 'true')
    })
    expect(screen.queryByTestId('timeline-period-2026-10-04T10')).toBeNull()
    expect(screen.getAllByTestId(/^timeline-item-/)).toHaveLength(3)
  })
})
