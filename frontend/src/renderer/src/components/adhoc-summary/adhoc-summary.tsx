// 任意时段总结：先预览（有多少内容、要花多少 token）再生成。
//
// 三层信息按「用户做决定」的顺序排：
//   1. 选范围 → 预览告诉用户「有什么、大概多少钱」；
//   2. 生成是**异步作业**：长范围要跑几十秒，界面必须能看进度、能取消，
//      否则用户只能对着一个不动的按钮猜；
//   3. 结果带质量标记：模型写的与模板兜底必须能区分 —— 混在一起显示，
//      用户就分不清「这条总结值不值得信」。

import { Button, Input, Message } from '@arco-design/web-react'
import { useI18n } from '@renderer/i18n'
import dayjs from 'dayjs'
import { FC, useCallback, useEffect, useRef, useState } from 'react'

import { rangeFromHash } from './range-from-url'

interface PreviewCounts {
  observations: number
  blocked_observations: number
  activities: number
  stages: number
}

interface Preview {
  counts: PreviewCounts
  estimated_chunks: number
  estimated_tokens: number
  has_data: boolean
  timezone?: string
}

interface JobSnapshot {
  job_id: string
  state: string
  chunks_done: number
  chunks_total: number
  summary_id?: string | null
  error?: string | null
}

interface SummaryRow {
  id: string
  title?: string
  body_markdown?: string
  quality?: string
}

interface AdhocApi {
  preview?: (from: string, to: string) => Promise<unknown>
  submit?: (from: string, to: string) => Promise<unknown>
  job?: (jobId: string) => Promise<unknown>
  cancel?: (jobId: string) => Promise<unknown>
}

function adhocApi(): AdhocApi | undefined {
  return (window as unknown as { adhocApi?: AdhocApi }).adhocApi
}

/** 范围里是否带时间（时间轴拖选与深链是分钟级；预设按钮只给日期）。 */
export function hasTimePrecision(value: string): boolean {
  return value.includes('T') || value.includes(' ')
}

/**
 * 把用户选的范围折成时间戳：纯日期折成一天的开头/结尾（与搜索页同一套口径），
 * 带时间的范围**原样保留** —— 折成整天会把用户选的 30 分钟悄悄变成一整天。
 */
export function dayRange(from: string, to: string): [string, string] {
  return [
    hasTimePrecision(from) ? dayjs(from).toISOString() : dayjs(from).startOf('day').toISOString(),
    hasTimePrecision(to) ? dayjs(to).toISOString() : dayjs(to).endOf('day').toISOString()
  ]
}

// 质量标记 → 词条 key：模块级常量用不了 hook，渲染时再 t()。
const QUALITY_LABEL_KEYS: Record<string, string> = {
  model: 'summary.quality.short.model',
  fallback: 'summary.quality.short.fallback'
}

export const AdhocSummary: FC = () => {
  const { t } = useI18n()
  const today = dayjs().format('YYYY-MM-DD')
  // 初始范围可以来自路由参数（时间轴拖选后深链过来，精度到分钟）
  const [initial] = useState(() =>
    rangeFromHash(window.location.hash, { from: dayjs().subtract(1, 'day').format('YYYY-MM-DD'), to: today })
  )
  const [from, setFrom] = useState(() =>
    (hasTimePrecision(initial.from) ? dayjs(initial.from) : dayjs(initial.from).startOf('day')).format(
      'YYYY-MM-DDTHH:mm:ss.SSS'
    )
  )
  const [to, setTo] = useState(() =>
    (hasTimePrecision(initial.to) ? dayjs(initial.to) : dayjs(initial.to).endOf('day')).format(
      'YYYY-MM-DDTHH:mm:ss.SSS'
    )
  )
  const [preview, setPreview] = useState<Preview | null>(null)
  const [restoredJobId] = useState(() => new URLSearchParams(window.location.hash.split('?')[1] ?? '').get('job_id'))
  const [job, setJob] = useState<JobSnapshot | null>(() =>
    restoredJobId
      ? {
          job_id: restoredJobId,
          state: 'running',
          chunks_done: 0,
          chunks_total: 1
        }
      : null
  )
  const [summary, setSummary] = useState<SummaryRow | null>(null)
  const [busy, setBusy] = useState(Boolean(restoredJobId))
  const [pollFailed, setPollFailed] = useState(false)
  const [retry, setRetry] = useState(0)
  const actionRef = useRef(false)
  const jobId = job?.job_id

  useEffect(() => {
    if (!jobId) return
    let disposed = false
    let timer: number | undefined
    const poll = async () => {
      try {
        const api = adhocApi()
        if (!api?.job) throw new Error('Job API unavailable')
        const payload = (await api.job(jobId)) as { job?: JobSnapshot; summary?: SummaryRow } | undefined
        if (disposed) return
        const snapshot = payload?.job
        if (!snapshot) throw new Error('Job response missing')
        setJob(snapshot)
        setPollFailed(false)
        if (['done', 'failed', 'cancelled'].includes(snapshot.state)) {
          setSummary(payload?.summary ?? null)
          setBusy(false)
        } else {
          timer = window.setTimeout(() => void poll(), 800)
        }
      } catch {
        if (!disposed) setPollFailed(true)
      }
    }
    void poll()
    return () => {
      disposed = true
      window.clearTimeout(timer)
    }
  }, [jobId, retry])

  const changeRange = (start: string, end: string) => {
    setFrom(start)
    setTo(end)
    setPreview(null)
    setSummary(null)
    setJob(null)
    setPollFailed(false)
  }

  const onPreview = useCallback(async () => {
    const api = adhocApi()
    if (busy || actionRef.current) return
    if (!api?.preview) {
      Message.error(t('summary.previewFailed'))
      return
    }
    actionRef.current = true
    setPreview(null)
    setBusy(true)
    try {
      const [start, end] = dayRange(from, to)
      if (start > end) throw new Error('Invalid range')
      const payload = (await api.preview(start, end)) as Preview | undefined
      setPreview(payload ?? null)
      setSummary(null)
      setJob(null)
    } catch (error) {
      Message.error(t('summary.previewFailed'))
    } finally {
      setBusy(false)
      actionRef.current = false
    }
  }, [busy, from, t, to])

  const onGenerate = useCallback(async () => {
    const api = adhocApi()
    if (busy || actionRef.current || !preview?.has_data) return
    if (!api?.submit) {
      Message.error(t('summary.generateFailed'))
      return
    }
    actionRef.current = true
    setBusy(true)
    setSummary(null)
    try {
      const [start, end] = dayRange(from, to)
      const submitted = (await api.submit(start, end)) as { job_id?: string; chunks_total?: number } | undefined
      const jobId = submitted?.job_id
      if (!jobId) throw new Error(t('summary.jobIdMissing'))
      setJob({
        job_id: jobId,
        state: 'running',
        chunks_done: 0,
        chunks_total: submitted?.chunks_total ?? 1
      })
      setPollFailed(false)
    } catch {
      Message.error(t('summary.generateFailed'))
      setBusy(false)
    } finally {
      actionRef.current = false
    }
  }, [busy, from, preview, t, to])

  const onCancel = useCallback(async () => {
    const api = adhocApi()
    if (!api?.cancel || !job) return
    try {
      await api.cancel(job.job_id)
    } catch {
      Message.error(t('summary.cancelFailed'))
    }
  }, [job, t])

  const running = job?.state === 'running' || job?.state === 'queued'

  return (
    <div className="adhoc-summary p-4">
      <div className="mb-2 flex gap-2">
        {/* 预设时段：手写日期是最烦的一步，常见区间直接给按钮 */}
        {(
          [
            [t('summary.preset.today'), 0, 0],
            [t('summary.preset.yesterday'), 1, 1],
            [t('summary.preset.last7'), 6, 0]
          ] as Array<[string, number, number]>
        ).map(([label, back, forward]) => (
          <Button
            key={label}
            size="small"
            disabled={busy}
            data-testid={`adhoc-preset-${label}`}
            onClick={() => {
              // 预设给「整天」：开始 00:00、结束 23:59，用户再按需改时刻
              changeRange(
                dayjs().subtract(back, 'day').startOf('day').format('YYYY-MM-DDTHH:mm'),
                dayjs().subtract(forward, 'day').endOf('day').format('YYYY-MM-DDTHH:mm:ss.SSS')
              )
            }}>
            {label}
          </Button>
        ))}
      </div>

      <div className="mb-3 flex items-end gap-2">
        <label className="flex flex-col text-xs text-[var(--color-text-3)]">
          {t('summary.from')}
          <Input
            aria-label={t('summary.fromLabel')}
            type="datetime-local"
            value={from.slice(0, 16)}
            disabled={busy}
            onChange={(value) => changeRange(value, to)}
            className="!w-[210px]"
          />
        </label>
        <label className="flex flex-col text-xs text-[var(--color-text-3)]">
          {t('summary.to')}
          <Input
            aria-label={t('summary.toLabel')}
            type="datetime-local"
            value={to.slice(0, 16)}
            disabled={busy}
            onChange={(value) => changeRange(from, value)}
            className="!w-[210px]"
          />
        </label>
        {/* 输入框只显示到分钟，这里把实际要跑的范围再写一遍（含跨天/时刻） */}
        <span data-testid="adhoc-precision-range" className="pb-1 text-xs text-[var(--color-text-3)]">
          {t('summary.selectedRange')}: {dayjs(from).format('MM-DD HH:mm')} – {dayjs(to).format('MM-DD HH:mm')}
        </span>
        <Button onClick={() => void onPreview()} disabled={busy} loading={busy && !running}>
          {t('summary.preview')}
        </Button>
      </div>

      {preview && (
        <div data-testid="adhoc-preview" className="mb-3 text-xs text-[var(--color-text-2)]">
          {preview.has_data ? (
            <>
              <div>
                {t('summary.preview.counts', {
                  observations: preview.counts.observations,
                  activities: preview.counts.activities,
                  stages: preview.counts.stages
                })}
                {preview.counts.blocked_observations > 0 && (
                  <> {t('summary.preview.blocked', { count: preview.counts.blocked_observations })}</>
                )}
              </div>
              <div>
                {t('summary.preview.estimate', {
                  chunks: preview.estimated_chunks,
                  tokens: preview.estimated_tokens
                })}
              </div>
            </>
          ) : (
            <div>{t('summary.preview.noData')}</div>
          )}
        </div>
      )}

      {job && running && (
        <div data-testid="adhoc-progress" className="mb-3 text-xs text-[var(--color-text-3)]">
          {t('summary.progress', { done: job.chunks_done, total: job.chunks_total })}
        </div>
      )}

      <div className="flex gap-2">
        <Button
          type="primary"
          className="!bg-[rgb(var(--primary-6))] !border-[rgb(var(--primary-6))]"
          disabled={!preview?.has_data || busy}
          loading={busy && running}
          onClick={() => void onGenerate()}>
          {t('summary.generate')}
        </Button>
        {running && <Button onClick={() => void onCancel()}>{t('summary.cancel')}</Button>}
      </div>

      {pollFailed && (
        <div role="alert">
          {t('summary.pollFailed')}
          <Button
            onClick={() => {
              setPollFailed(false)
              setRetry((value) => value + 1)
            }}>
            {t('common.retry')}
          </Button>
        </div>
      )}
      {job?.state === 'failed' && <p role="alert">{job.error || t('summary.generateFailed')}</p>}
      {job?.state === 'cancelled' && <p role="status">{t('summary.cancelled')}</p>}

      {summary && (
        <div className="mt-3 rounded-[8px] bg-[var(--color-bg-4)] p-3">
          <div className="flex items-center gap-2">
            <span className="text-sm font-bold">{summary.title}</span>
            {summary.quality && (
              <span
                data-testid="adhoc-quality"
                className="rounded-[4px] bg-white px-[6px] text-[10px] text-[var(--color-text-3)]">
                {t(QUALITY_LABEL_KEYS[summary.quality] ?? summary.quality)}
              </span>
            )}
          </div>
          <div className="mt-1 whitespace-pre-wrap text-xs text-[var(--color-text-2)]">{summary.body_markdown}</div>
        </div>
      )}
    </div>
  )
}
