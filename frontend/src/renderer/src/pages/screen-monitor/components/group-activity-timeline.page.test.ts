// 页面级测试（无需 DOM）：活动列表 → 按时段 / 分类折叠的时间线分组。
//
// 交付契约：用户先看到「几点钟 · 哪一类工作」，而不是一堵截图墙。

import { describe, expect, it } from 'vitest'

import { categoryFromMetadata, groupActivityTimeline } from './group-activity-timeline'

const act = (id: string, start: string, end: string, title: string, category: string | null = null, images = 0) => ({
  id,
  start_time: start,
  end_time: end,
  title,
  category,
  resources: Array.from({ length: images }, (_, i) => ({
    type: 'image',
    id: `${id}-${i}`,
    path: `shots/${id}-${i}.png`
  }))
})

describe('screen-monitor/group-activity-timeline', () => {
  it('空列表得到空分组', () => {
    expect(groupActivityTimeline([])).toEqual([])
  })

  it('按时段（小时）倒序折叠，同小时内按分类汇总', () => {
    const periods = groupActivityTimeline(
      [
        act('a', '2026-10-04 10:10:00', '2026-10-04 10:25:00', '写导入脚本', '开发', 2),
        act('b', '2026-10-04 10:30:00', '2026-10-04 10:50:00', '修测试', '开发', 1),
        act('c', '2026-10-04 09:05:00', '2026-10-04 09:40:00', '需求评审', '需求', 3),
        act('d', '2026-10-04 09:45:00', '2026-10-04 09:55:00', '刷邮件', null, 1)
      ],
      { uncategorizedLabel: '未分类' }
    )

    expect(periods.map((p) => p.key)).toEqual(['2026-10-04T10', '2026-10-04T09'])
    expect(periods[0].activityCount).toBe(2)
    expect(periods[0].screenshotCount).toBe(3)
    expect(periods[0].groups.map((g) => g.label)).toEqual(['开发'])
    expect(periods[0].groups[0].activities.map((a) => a.id)).toEqual(['b', 'a'])

    // 同小时内按组内最晚结束倒序：09:55 的未分类排在 09:40 的需求前面
    expect(periods[1].groups.map((g) => g.label)).toEqual(['未分类', '需求'])
    expect(periods[1].groups[1].screenshotCount).toBe(3)
    expect(periods[1].groups[0].label).toBe('未分类')
  })

  it('分类优先用显式 category，空串与空白视为未分类', () => {
    const periods = groupActivityTimeline(
      [
        act('a', '2026-10-04 11:00:00', '2026-10-04 11:10:00', 'Chrome', '  ', 0),
        act('b', '2026-10-04 11:15:00', '2026-10-04 11:20:00', '终端', '', 0)
      ],
      { uncategorizedLabel: '未分类' }
    )
    expect(periods).toHaveLength(1)
    expect(periods[0].groups).toHaveLength(1)
    expect(periods[0].groups[0].label).toBe('未分类')
    expect(periods[0].groups[0].activities).toHaveLength(2)
  })

  it('分组起止取组内最早开始与最晚结束，时长为各活动时长之和', () => {
    const periods = groupActivityTimeline([
      act('a', '2026-10-04 14:00:00', '2026-10-04 14:10:00', 'A', '开发', 0),
      act('b', '2026-10-04 14:20:00', '2026-10-04 14:35:00', 'B', '开发', 0)
    ])
    const group = periods[0].groups[0]
    expect(group.start_time).toBe('2026-10-04 14:00:00')
    expect(group.end_time).toBe('2026-10-04 14:35:00')
    // 10 + 15 分钟
    expect(group.durationMs).toBe(25 * 60 * 1000)
  })

  it('从兼容面 metadata JSON 取出 category，坏数据返回 null', () => {
    expect(categoryFromMetadata('{"category":"开发","projector":"activities"}')).toBe('开发')
    expect(categoryFromMetadata('{"category":"  "}')).toBeNull()
    expect(categoryFromMetadata('{')).toBeNull()
    expect(categoryFromMetadata(null)).toBeNull()
  })
})
