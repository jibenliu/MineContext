// 把扁平活动列表折成「时段 → 分类」两级时间线。
//
// 为什么要这一层：投影结果已经是活动（不是逐帧截图），但界面仍按活动平铺截图，
// 一天下来就是一堵墙。先按小时、再按 category 汇总，用户才看得到「几点在做什么类事」。
// category 来自投影 / v1；没有分类时归入调用方给的未分类标签，不猜应用名。

import dayjs from 'dayjs'

export interface TimelineActivityLike {
  id: string
  start_time: string
  end_time: string
  title: string
  category?: string | null
  resources?: Array<{ type?: string }>
}

export interface ActivityAppGroup<T extends TimelineActivityLike = TimelineActivityLike> {
  /** 分组键：规范化后的分类，或未分类哨兵 */
  key: string
  label: string
  activities: T[]
  screenshotCount: number
  start_time: string
  end_time: string
  durationMs: number
}

export interface ActivityPeriodGroup<T extends TimelineActivityLike = TimelineActivityLike> {
  /** 本地小时桶，形如 `2026-10-04T10` */
  key: string
  start_time: string
  end_time: string
  groups: ActivityAppGroup<T>[]
  activityCount: number
  screenshotCount: number
}

export interface GroupActivityTimelineOptions {
  uncategorizedLabel?: string
}

const UNCATEGORIZED_KEY = '__uncategorized__'

function screenshotCountOf(activity: TimelineActivityLike): number {
  return (activity.resources || []).filter((resource) => resource.type === 'image').length
}

function categoryKey(activity: TimelineActivityLike): string {
  const raw = activity.category?.trim()
  return raw ? raw : UNCATEGORIZED_KEY
}

function categoryLabel(key: string, uncategorizedLabel: string): string {
  return key === UNCATEGORIZED_KEY ? uncategorizedLabel : key
}

function periodKey(startTime: string): string {
  return dayjs(startTime).format('YYYY-MM-DDTHH')
}

function durationMsOf(activity: TimelineActivityLike): number {
  const start = dayjs(activity.start_time).valueOf()
  const end = dayjs(activity.end_time).valueOf()
  if (Number.isNaN(start) || Number.isNaN(end) || end < start) return 0
  return end - start
}

export function groupActivityTimeline<T extends TimelineActivityLike>(
  activities: T[],
  options: GroupActivityTimelineOptions = {}
): ActivityPeriodGroup<T>[] {
  const uncategorizedLabel = options.uncategorizedLabel ?? '未分类'
  if (activities.length === 0) return []

  const sorted = [...activities].sort((a, b) => dayjs(b.start_time).valueOf() - dayjs(a.start_time).valueOf())

  const periodOrder: string[] = []
  const byPeriod = new Map<string, T[]>()
  for (const activity of sorted) {
    const key = periodKey(activity.start_time)
    if (!byPeriod.has(key)) {
      periodOrder.push(key)
      byPeriod.set(key, [])
    }
    byPeriod.get(key)!.push(activity)
  }

  return periodOrder.map((key) => {
    const inPeriod = byPeriod.get(key) ?? []
    const categoryOrder: string[] = []
    const byCategory = new Map<string, T[]>()
    for (const activity of inPeriod) {
      const cat = categoryKey(activity)
      if (!byCategory.has(cat)) {
        categoryOrder.push(cat)
        byCategory.set(cat, [])
      }
      byCategory.get(cat)!.push(activity)
    }

    // 同小时内分类按组内最晚结束倒序：最近还在做的类排上面
    categoryOrder.sort((a, b) => {
      const aEnd = Math.max(...(byCategory.get(a) ?? []).map((item) => dayjs(item.end_time).valueOf()))
      const bEnd = Math.max(...(byCategory.get(b) ?? []).map((item) => dayjs(item.end_time).valueOf()))
      return bEnd - aEnd
    })

    const groups: ActivityAppGroup<T>[] = categoryOrder.map((cat) => {
      const items = byCategory.get(cat) ?? []
      const starts = items.map((item) => item.start_time)
      const ends = items.map((item) => item.end_time)
      const start_time = starts.reduce((earliest, next) => (dayjs(next).isBefore(dayjs(earliest)) ? next : earliest))
      const end_time = ends.reduce((latest, next) => (dayjs(next).isAfter(dayjs(latest)) ? next : latest))
      return {
        key: cat,
        label: categoryLabel(cat, uncategorizedLabel),
        activities: items,
        screenshotCount: items.reduce((sum, item) => sum + screenshotCountOf(item), 0),
        start_time,
        end_time,
        durationMs: items.reduce((sum, item) => sum + durationMsOf(item), 0)
      }
    })

    return {
      key,
      start_time: groups.reduce(
        (earliest, group) => (dayjs(group.start_time).isBefore(dayjs(earliest)) ? group.start_time : earliest),
        groups[0].start_time
      ),
      end_time: groups.reduce(
        (latest, group) => (dayjs(group.end_time).isAfter(dayjs(latest)) ? group.end_time : latest),
        groups[0].end_time
      ),
      groups,
      activityCount: inPeriod.length,
      screenshotCount: groups.reduce((sum, group) => sum + group.screenshotCount, 0)
    }
  })
}

/** 从兼容面 `metadata` JSON 字符串里取出 category；解析失败或没有字段时返回 null。 */
export function categoryFromMetadata(metadata: unknown): string | null {
  if (typeof metadata !== 'string' || !metadata.trim()) return null
  try {
    const parsed = JSON.parse(metadata) as { category?: unknown }
    if (typeof parsed.category === 'string' && parsed.category.trim()) {
      return parsed.category.trim()
    }
    return null
  } catch {
    return null
  }
}
