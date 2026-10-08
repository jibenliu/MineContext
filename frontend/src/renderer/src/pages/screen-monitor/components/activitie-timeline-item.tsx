// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import { Image, Input, Popover, Tooltip } from '@arco-design/web-react'
import { useI18n } from '@renderer/i18n'
import { FC, useState } from 'react'

import type { ProvenanceBadge } from '../hooks/use-activity-provenance'
import { Activity } from '../screen-monitor'
import { ScreenshotImage } from './screenshot-image'

export interface ActivityTimelineItemProps {
  activity: Activity
  /** 结论来源；拿不到扩展面时为 null（不标，而不是猜） */
  provenance?: ProvenanceBadge | null
  /** 用户改名；不给就不显示改名入口（例如拿不到扩展面的外壳） */
  onRename?: (title: string) => Promise<void> | void
  /** 并入时间上的上一条活动（合并修正）；不给就不显示入口 */
  onMergeInto?: () => Promise<void> | void
  /** 在某时刻把这条活动切成两段；参数是毫秒时间戳与后半段标题 */
  onSplit?: (atMs: number, tailTitle: string) => Promise<void> | void
}
const ActivityTimelineItem: FC<ActivityTimelineItemProps> = (props) => {
  const { activity, provenance, onRename, onMergeInto, onSplit } = props
  const { t } = useI18n()
  const [editing, setEditing] = useState(false)
  const [splitting, setSplitting] = useState(false)
  const [splitTitle, setSplitTitle] = useState('')
  const [draft, setDraft] = useState(activity.title)

  const commit = async (): Promise<void> => {
    const title = draft.trim()
    setEditing(false)
    if (!title || title === activity.title) return
    await onRename?.(title)
  }

  return (
    <div className="pb-[24px] text-[14px]">
      <div className="flex items-center gap-2 pb-[12px]">
        {editing ? (
          <Input
            size="small"
            autoFocus
            aria-label="活动标题"
            value={draft}
            onChange={setDraft}
            // 用 keydown 而不是 onPressEnter：后者依赖 keypress 事件，
            // 在 jsdom 与部分输入法组合下不触发，改名会「按了回车没反应」
            onKeyDown={(event) => {
              if (event.key === 'Enter') void commit()
              if (event.key === 'Escape') setEditing(false)
            }}
            onBlur={() => void commit()}
            className="max-w-[280px]"
          />
        ) : (
          <Popover
            content={activity.content}
            trigger="hover"
            triggerProps={{ position: 'tl' }}
            disabled={!activity.content}>
            <div className="cursor-pointer font-bold text-[12px] text-[var(--color-text-1)] hover:font-extrabold">
              {activity.title}
            </div>
          </Popover>
        )}
        {provenance && (
          <Tooltip content={provenance.detail}>
            <span
              data-testid={`provenance-${activity.id}`}
              title={provenance.detail}
              className="rounded-[4px] bg-[var(--color-fill-2)] px-[6px] text-[10px] text-[var(--color-text-3)]">
              {provenance.label}
            </span>
          </Tooltip>
        )}
        {onSplit && !editing && !splitting && (
          <button
            type="button"
            data-testid={`split-${activity.id}`}
            title={t('screenMonitor.activity.splitTitle')}
            className="cursor-pointer text-[10px] text-[var(--color-text-3)]"
            onClick={() => {
              setSplitTitle(`${activity.title}${t('screenMonitor.activity.continuation')}`)
              setSplitting(true)
            }}>
            {t('screenMonitor.activity.split')}
          </button>
        )}
        {onMergeInto && !editing && (
          <button
            type="button"
            data-testid={`merge-${activity.id}`}
            title={t('screenMonitor.activity.mergeTitle')}
            className="cursor-pointer text-[10px] text-[var(--color-text-3)]"
            onClick={() => void onMergeInto()}>
            {t('screenMonitor.activity.merge')}
          </button>
        )}
        {onRename && !editing && provenance?.kind === 'inferred' && (
          <button
            type="button"
            data-testid={`rename-${activity.id}`}
            className="cursor-pointer text-[10px] text-[#5252FF]"
            onClick={() => {
              setDraft(activity.title)
              setEditing(true)
            }}>
            {t('screenMonitor.activity.rename')}
          </button>
        )}
      </div>
      {splitting && onSplit && (
        <div className="mb-2 flex items-center gap-2">
          <Input
            size="small"
            type="time"
            aria-label="拆分时刻"
            defaultValue="12:00"
            id={`split-at-${activity.id}`}
            className="w-[110px]"
          />
          <Input
            size="small"
            aria-label="后半段标题"
            value={splitTitle}
            onChange={setSplitTitle}
            className="max-w-[220px]"
          />
          <button
            type="button"
            data-testid={`split-confirm-${activity.id}`}
            className="cursor-pointer text-[10px] text-[#5252FF]"
            onClick={() => {
              const input = document.getElementById(`split-at-${activity.id}`) as HTMLInputElement | null
              const raw = input?.value ?? ''
              const title = splitTitle.trim()
              setSplitting(false)
              if (!raw || !title) return
              // 时刻换算成毫秒：取这条活动开始那天的 `HH:mm`（本地时区）
              const base = new Date(activity.start_time.replace(' ', 'T'))
              const [hour, minute] = raw.split(':')
              base.setHours(Number(hour), Number(minute), 0, 0)
              void onSplit(base.getTime(), title)
            }}>
            {t('screenMonitor.activity.confirmSplit')}
          </button>
        </div>
      )}
      <div className="screenshots-container flex flex-wrap align-center gap-2">
        <Image.PreviewGroup infinite className="[&_.arco-image-preview-img]:!scale-80">
          {(activity?.resources || [])
            .filter((resource) => resource.type === 'image')
            .map((resource, index) => {
              return (
                <ScreenshotImage
                  key={resource.id ?? index}
                  path={resource.path}
                  alt={`screenshot-${index + 1}`}
                  index={index}
                />
              )
            })}
        </Image.PreviewGroup>
      </div>
    </div>
  )
}
export { ActivityTimelineItem }
