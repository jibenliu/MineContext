// 页面级测试：时间轴拖选 —— 按下 → 划过 → 松开 → 总结所选时段。
//
// 这里钉的是**鼠标事件的连接本身**（范围折算规则由 selection-range 的测试覆盖）：
// 没有这条，之前只能靠代码审阅保证事件挂对了元素。

import { installFakeBackend } from '@renderer/test/page-setup'
import { fireEvent, render, screen } from '@testing-library/react'
import dayjs from 'dayjs'
import { describe, expect, it, vi } from 'vitest'

import { RecordingTimeline } from './components/recording-timeline'

vi.mock('./components/activitie-timeline-item', () => ({
  ActivityTimelineItem: ({ activity }: { activity: { title: string } }) => <span>{activity.title}</span>
}))

const activity = (id: number, start: string, end: string) => ({
  id: String(id),
  start_time: start,
  end_time: end,
  title: `活动 ${id}`,
  content: '',
  resources: []
})

// 时间线内部按开始时间倒序渲染：下标 0 是最新的一条
const items = [
  activity(11, '2026-10-04 10:00:00', '2026-10-04 10:30:00'),
  activity(12, '2026-10-04 09:00:00', '2026-10-04 09:30:00'),
  activity(13, '2026-10-04 08:00:00', '2026-10-04 08:30:00')
]

function renderTimeline(onSummarizeRange: (from: string, to: string) => void) {
  installFakeBackend({}, { strict: false })
  render(
    <RecordingTimeline
      isMonitoring={false}
      isToday={false}
      canRecord={false}
      activities={items}
      recordingStats={null}
      onSummarizeRange={onSummarizeRange}
    />
  )
}

const iso = (value: string) => dayjs(value).toISOString()

describe('时间线拖选 → 总结所选时段', () => {
  it('长列表每页最多 50 条，翻页清除旧选择，下一页使用正确的活动范围', async () => {
    installFakeBackend({}, { strict: false })
    const onSummarizeRange = vi.fn()
    const many = Array.from({ length: 51 }, (_, index) => {
      const start = dayjs('2026-10-04 00:00:00').add(index, 'minute')
      return activity(index, start.toISOString(), start.add(1, 'minute').toISOString())
    })
    render(
      <RecordingTimeline
        isMonitoring={false}
        isToday={false}
        canRecord={false}
        activities={many}
        recordingStats={null}
        onSummarizeRange={onSummarizeRange}
      />
    )
    expect(screen.getAllByTestId(/^timeline-item-/)).toHaveLength(50)
    fireEvent.mouseDown(screen.getByTestId('timeline-item-0'))
    fireEvent.mouseUp(screen.getByTestId('timeline-item-0'))
    expect(screen.getByTestId('timeline-selection')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: '下一页' }))
    expect(screen.getAllByTestId(/^timeline-item-/)).toHaveLength(1)
    expect(screen.queryByTestId('timeline-selection')).toBeNull()
    fireEvent.mouseDown(screen.getByTestId('timeline-item-50'))
    fireEvent.mouseUp(screen.getByTestId('timeline-item-50'))
    fireEvent.click(screen.getByTestId('summarize-selection'))
    expect(onSummarizeRange).toHaveBeenCalledWith(many[0].start_time, many[0].end_time)
  })
  it('从最新一条拖到最早一条：拿到跨三条的完整范围', async () => {
    const onSummarizeRange = vi.fn()
    renderTimeline(onSummarizeRange)

    fireEvent.mouseDown(screen.getByTestId('timeline-item-0'))
    fireEvent.mouseEnter(screen.getByTestId('timeline-item-2'))
    fireEvent.mouseUp(screen.getByTestId('timeline-item-2'))

    const bar = await screen.findByTestId('timeline-selection')
    expect(bar.textContent).toContain('08:00')
    expect(bar.textContent).toContain('10:30')

    fireEvent.click(screen.getByTestId('summarize-selection'))
    expect(onSummarizeRange).toHaveBeenCalledWith(iso('2026-10-04 08:00:00'), iso('2026-10-04 10:30:00'))
  })

  it('只选一条：范围就是那一条，不扩成整天', async () => {
    const onSummarizeRange = vi.fn()
    renderTimeline(onSummarizeRange)

    fireEvent.mouseDown(screen.getByTestId('timeline-item-1'))
    fireEvent.mouseUp(screen.getByTestId('timeline-item-1'))
    fireEvent.click(await screen.findByTestId('summarize-selection'))

    expect(onSummarizeRange).toHaveBeenCalledWith(iso('2026-10-04 09:00:00'), iso('2026-10-04 09:30:00'))
  })

  it('还没选之前不显示动作，不给用户一个点了没反应的按钮', () => {
    renderTimeline(vi.fn())

    expect(screen.queryByTestId('timeline-selection')).toBeNull()
    expect(screen.queryByTestId('summarize-selection')).toBeNull()
  })
})
