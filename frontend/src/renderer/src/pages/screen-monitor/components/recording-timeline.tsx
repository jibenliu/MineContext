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
import RecordingStatsCard, { RecordingStats } from './recording-stats-card'
import { selectionRange } from './selection-range'

const { Text } = Typography
const TimelineItem = Timeline.Item
const PAGE_SIZE = 50

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

  // 拖选：按下记起点、划过延伸、松开定范围。列表是倒序，方向交给
  // selectionRange 处理（它按时间取最早开始与最晚结束）
  const [anchor, setAnchor] = React.useState<number | null>(null)
  const [hovered, setHovered] = React.useState<number | null>(null)
  const [picked, setPicked] = React.useState<{ from: string; to: string } | null>(null)
  const [requestedPage, setRequestedPage] = React.useState(0)

  // 排序用副本：直接 sort(props) 会改调用方的数组
  const sorted = React.useMemo(
    () => [...activities].sort((a, b) => dayjs(b.start_time).valueOf() - dayjs(a.start_time).valueOf()),
    [activities]
  )
  const pages = Math.max(1, Math.ceil(sorted.length / PAGE_SIZE))
  const page = Math.min(requestedPage, pages - 1)
  const offset = page * PAGE_SIZE
  const changePage = (next: number) => {
    setAnchor(null)
    setHovered(null)
    setPicked(null)
    setRequestedPage(next)
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

  return (
    <div className="mt-5">
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

        {/* Display activities */}
        {sorted.slice(offset, offset + PAGE_SIZE).map((activity, localIndex) => {
          const index = offset + localIndex
          // 列表是倒序：时间上「上一条」是数组里的下一个
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
                  key={activity.id}
                  activity={activity}
                  provenance={provenance.badgeFor(activity.id)}
                  onRename={(title) => provenance.rename(activity.id, title)}
                  onMergeInto={earlier ? () => provenance.merge(earlier.id, activity.id) : undefined}
                  onSplit={(atMs, tailTitle) => provenance.split(activity.id, atMs, tailTitle)}
                />
              </div>
            </TimelineItem>
          )
        })}
      </Timeline>
      <ListPagination page={page} pages={pages} onChange={changePage} />
    </div>
  )
}

export default RecordingTimeline
export { RecordingTimeline }
