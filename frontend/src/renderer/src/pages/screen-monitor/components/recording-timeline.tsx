import { Timeline, Typography } from '@arco-design/web-react'
import { ListPagination } from '@renderer/components/list-pagination'
import { useI18n } from '@renderer/i18n'
import { formatTime } from '@renderer/utils/time'
import dayjs from 'dayjs'
import React from 'react'

import { SCREEN_INTERVAL_TIME } from '../constant'
import { useActivityProvenance } from '../hooks/use-activity-provenance'
import { Activity } from '../screen-monitor'
import { ActivityTimelineItem } from './activitie-timeline-item'
import { categoryFromMetadata, groupActivityTimeline } from './group-activity-timeline'
import RecordingStatsCard, { RecordingStats } from './recording-stats-card'
import { selectionRange } from './selection-range'

const { Text } = Typography
const TimelineItem = Timeline.Item
const PAGE_SIZE = 50

type TimelineView = 'grouped' | 'list'

interface RecordingTimelineProps {
  isMonitoring: boolean
  isToday: boolean
  canRecord: boolean
  activities: Activity[]
  recordingStats: RecordingStats | null
  /** 不能录制时的原因（后端 /api/capture/status 的 reason） */
  recordReason?: string
  onSummarizeRange?: (from: string, to: string) => void
}

function formatDuration(ms: number): string {
  const totalMinutes = Math.max(0, Math.round(ms / 60000))
  if (totalMinutes < 60) return `${totalMinutes}m`
  const hours = Math.floor(totalMinutes / 60)
  const minutes = totalMinutes % 60
  return minutes === 0 ? `${hours}h` : `${hours}h ${minutes}m`
}

const RecordingTimeline: React.FC<RecordingTimelineProps> = ({
  isMonitoring,
  isToday,
  canRecord,
  activities,
  recordingStats,
  recordReason,
  onSummarizeRange
}) => {
  const { t } = useI18n()
  // 结论来源与改名都来自扩展面；拿不到就只少一个标记，时间线照常渲染
  const provenance = useActivityProvenance()
  const [view, setView] = React.useState<TimelineView>('grouped')

  // 拖选：按下记起点、划过延伸、松开定范围。列表是倒序，方向交给
  // selectionRange 处理（它按时间取最早开始与最晚结束）
  const [anchor, setAnchor] = React.useState<number | null>(null)
  const [hovered, setHovered] = React.useState<number | null>(null)
  const [picked, setPicked] = React.useState<{ from: string; to: string } | null>(null)
  const [requestedPage, setRequestedPage] = React.useState(0)

  // 排序用副本：直接 sort(props) 会改调用方的数组
  const sorted = React.useMemo(
    () =>
      [...activities]
        .map((activity) => ({
          ...activity,
          category:
            activity.category?.trim() ||
            provenance.categoryFor(activity.id) ||
            categoryFromMetadata(activity.metadata) ||
            null
        }))
        .sort((a, b) => dayjs(b.start_time).valueOf() - dayjs(a.start_time).valueOf()),
    [activities, provenance.rows]
  )
  const pages = Math.max(1, Math.ceil(sorted.length / PAGE_SIZE))
  const page = Math.min(requestedPage, pages - 1)
  const offset = page * PAGE_SIZE
  const pageItems = sorted.slice(offset, offset + PAGE_SIZE)
  const periods = React.useMemo(
    () =>
      groupActivityTimeline(pageItems, {
        uncategorizedLabel: t('screenMonitor.timeline.uncategorized')
      }),
    [pageItems, t]
  )

  const changePage = (next: number) => {
    setAnchor(null)
    setHovered(null)
    setPicked(null)
    setRequestedPage(next)
  }

  const changeView = (next: TimelineView) => {
    setView(next)
    setAnchor(null)
    setHovered(null)
    setPicked(null)
  }

  const isSelected = (index: number): boolean => {
    if (anchor === null) return false
    const other = hovered ?? anchor
    return index >= Math.min(anchor, other) && index <= Math.max(anchor, other)
  }

  const commitSelection = (index: number): void => {
    if (anchor === null) return
    setPicked(selectionRange(sorted, anchor, index))
    setHovered(null)
    setAnchor(null)
  }

  const renderActivity = (activity: Activity, index: number, compact: boolean) => {
    const earlier = sorted[index + 1]
    return (
      <TimelineItem label={formatTime(activity?.end_time)} key={activity.id}>
        <div
          data-testid={`timeline-item-${index}`}
          onMouseDown={() => setAnchor(index)}
          onMouseEnter={() => setHovered(index)}
          onMouseUp={() => commitSelection(index)}
          // 选中底色走语义变量：写死浅紫在暗色主题下会亮成一块，与整页割裂
          className={isSelected(index) ? 'rounded-[8px] bg-[var(--color-primary-light-1)]' : undefined}>
          <ActivityTimelineItem
            activity={activity}
            compactScreenshots={compact}
            provenance={provenance.badgeFor(activity.id)}
            onRename={(title) => provenance.rename(activity.id, title)}
            onMergeInto={earlier ? () => provenance.merge(earlier.id, activity.id) : undefined}
            onSplit={(atMs, tailTitle) => provenance.split(activity.id, atMs, tailTitle)}
          />
        </div>
      </TimelineItem>
    )
  }

  return (
    <div className="mt-5">
      <div className="mb-3 flex items-center gap-2 text-xs" role="group" aria-label={t('screenMonitor.timeline.view')}>
        <button
          type="button"
          data-testid="timeline-view-grouped"
          aria-pressed={view === 'grouped'}
          className={
            view === 'grouped'
              ? 'rounded-[4px] bg-[var(--color-fill-2)] px-2 py-1 text-[var(--color-text-1)]'
              : 'rounded-[4px] px-2 py-1 text-[var(--color-text-3)]'
          }
          onClick={() => changeView('grouped')}>
          {t('screenMonitor.timeline.viewGrouped')}
        </button>
        <button
          type="button"
          data-testid="timeline-view-list"
          aria-pressed={view === 'list'}
          className={
            view === 'list'
              ? 'rounded-[4px] bg-[var(--color-fill-2)] px-2 py-1 text-[var(--color-text-1)]'
              : 'rounded-[4px] px-2 py-1 text-[var(--color-text-3)]'
          }
          onClick={() => changeView('list')}>
          {t('screenMonitor.timeline.viewList')}
        </button>
      </div>
      {picked && onSummarizeRange ? (
        <div
          data-testid="timeline-selection"
          className="mb-3 flex items-center gap-3 text-xs text-[var(--color-text-2)]">
          <span>
            {t('screenMonitor.selection.selected', {
              from: dayjs(picked.from).format('MM-DD HH:mm'),
              to: dayjs(picked.to).format('MM-DD HH:mm')
            })}
          </span>
          <button
            type="button"
            data-testid="summarize-selection"
            className="text-[#5252FF] underline"
            onClick={() => onSummarizeRange(picked.from, picked.to)}>
            {t('screenMonitor.selection.summarize')}
          </button>
        </div>
      ) : null}
      <Timeline labelPosition="relative">
        {isToday && (
          <TimelineItem label={t('screenMonitor.timeline.now')} className="!pb-[24px]">
            {isMonitoring ? (
              canRecord ? (
                <>
                  <div className="w-full text-sm">
                    <Text className="[&_.arco-typography]: !font-bold [&_.arco-typography]: !text-[#5252FF] [&_.arco-typography]: !text-xs">
                      {t('screenMonitor.timeline.recordingScreen')}
                    </Text>
                    <div className="text-[var(--color-text-4)]">
                      {t('screenMonitor.timeline.activityIntervalHint', { minutes: SCREEN_INTERVAL_TIME })}
                    </div>
                  </div>
                  <RecordingStatsCard stats={recordingStats} />
                </>
              ) : (
                <div className="w-full text-sm">
                  <Text className="[&_.arco-typography]: !font-bold [&_.arco-typography]: !text-[rgb(var(--danger-6))] [&_.arco-typography]: !text-xs">
                    {t('screenMonitor.timeline.recordingUnavailable')}
                  </Text>
                  {/* 「不能录制」与「不在录制时段」是两回事：前者要用户去处理（授权），
                      后者等一会儿就好 —— 不能共用一句「不在录制时段」 */}
                  <div className="text-[var(--color-text-4)]" data-testid="recording-unavailable-reason">
                    {recordReason || t('screenMonitor.timeline.recordingUnavailableReason')}
                  </div>
                </div>
              )
            ) : (
              <div style={{ width: '100%', fontSize: 14 }}>
                <Text style={{ fontWeight: 'bold', color: 'rgb(var(--danger-6))', fontSize: 12 }}>
                  {t('screenMonitor.timeline.recordingStopped')}
                </Text>
                <div style={{ color: 'var(--color-text-3)' }}>{t('screenMonitor.timeline.canStartAgain')}</div>
              </div>
            )}
          </TimelineItem>
        )}

        {/* Arco Timeline 只认直接子级的 TimelineItem；Fragment 会被丢掉，分组必须展平 */}
        {view === 'grouped'
          ? periods.flatMap((period) => [
              <TimelineItem key={`period-${period.key}`} label={dayjs(period.start_time).format('HH:00')}>
                <div data-testid={`timeline-period-${period.key}`} className="mb-2 text-xs text-[var(--color-text-2)]">
                  <span className="font-bold text-[var(--color-text-1)]">
                    {dayjs(period.start_time).format('HH:00')} –{' '}
                    {dayjs(period.start_time).add(1, 'hour').format('HH:00')}
                  </span>
                  <span className="ml-2">
                    {t('screenMonitor.timeline.periodSummary', {
                      activities: period.activityCount,
                      screenshots: period.screenshotCount
                    })}
                  </span>
                </div>
              </TimelineItem>,
              ...period.groups.flatMap((group) => [
                <TimelineItem key={`group-${period.key}-${group.key}`} label={formatTime(group.end_time)}>
                  <div
                    data-testid={`timeline-group-${period.key}-${group.label}`}
                    className="mb-1 rounded-[6px] bg-[var(--color-fill-1)] px-2 py-1.5 text-xs text-[var(--color-text-2)]">
                    <span className="font-bold text-[var(--color-text-1)]">{group.label}</span>
                    <span className="ml-2">
                      {dayjs(group.start_time).format('HH:mm')} – {dayjs(group.end_time).format('HH:mm')}
                    </span>
                    <span className="ml-2">{formatDuration(group.durationMs)}</span>
                    <span className="ml-2">
                      {t('screenMonitor.timeline.groupSummary', {
                        activities: group.activities.length,
                        screenshots: group.screenshotCount
                      })}
                    </span>
                  </div>
                </TimelineItem>,
                ...group.activities.map((activity) => {
                  const index = sorted.findIndex((item) => item.id === activity.id)
                  return renderActivity(activity, index, true)
                })
              ])
            ])
          : pageItems.map((activity, localIndex) => renderActivity(activity, offset + localIndex, false))}
      </Timeline>
      <ListPagination page={page} pages={pages} onChange={changePage} />
    </div>
  )
}

export default RecordingTimeline
export { RecordingTimeline }
