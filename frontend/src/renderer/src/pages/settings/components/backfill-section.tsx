// 设置页「补推断」：对历史未判出的活动入队一次补偿作业。
// 后端 jobs API 已就绪；这里只提供 from/to + 提交 + 轮询状态。

import { Button, DatePicker, Message, Typography } from '@arco-design/web-react'
import { useI18n } from '@renderer/i18n'
import dayjs, { type Dayjs } from 'dayjs'
import { useCallback, useEffect, useRef, useState } from 'react'

const { Text } = Typography

export interface JobsApi {
  enqueueBackfill: (from: string, to: string) => Promise<{ job_id?: number; deduped?: boolean } | undefined>
  jobStatus: (jobId: number) => Promise<
    | {
        id?: number
        state?: string
        skip_reason?: string | null
        last_error?: string | null
      }
    | undefined
  >
}

function createJobsApi(): JobsApi {
  const api = (globalThis as { jobsApi?: JobsApi }).jobsApi
  if (!api) {
    return {
      enqueueBackfill: async () => undefined,
      jobStatus: async () => undefined
    }
  }
  return api
}

const TERMINAL = new Set(['succeeded', 'failed', 'skipped', 'cancelled'])

export function BackfillSection({ api }: { api?: JobsApi }) {
  const { t } = useI18n()
  const [client] = useState(() => api ?? createJobsApi())
  const [from, setFrom] = useState<Dayjs>(() => dayjs().subtract(6, 'day').startOf('day'))
  const [to, setTo] = useState<Dayjs>(() => dayjs().endOf('day'))
  const [pending, setPending] = useState(false)
  const [status, setStatus] = useState('')
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null)

  const stopPolling = useCallback(() => {
    if (pollRef.current) {
      clearInterval(pollRef.current)
      pollRef.current = null
    }
  }, [])

  useEffect(() => () => stopPolling(), [stopPolling])

  const pollJob = useCallback(
    (jobId: number) => {
      stopPolling()
      const tick = async () => {
        try {
          const job = await client.jobStatus(jobId)
          const state = job?.state ?? 'unknown'
          if (job?.skip_reason) {
            setStatus(t('settings.backfill.statusSkipped', { reason: job.skip_reason }))
          } else if (job?.last_error) {
            setStatus(t('settings.backfill.statusFailed', { error: job.last_error }))
          } else {
            setStatus(t('settings.backfill.status', { state }))
          }
          if (TERMINAL.has(state)) {
            stopPolling()
            setPending(false)
          }
        } catch {
          setStatus(t('settings.backfill.pollFailed'))
          stopPolling()
          setPending(false)
        }
      }
      void tick()
      pollRef.current = setInterval(() => {
        void tick()
      }, 1500)
    },
    [client, stopPolling, t]
  )

  const onSubmit = useCallback(async () => {
    if (!from || !to || !from.isBefore(to)) {
      Message.error(t('settings.backfill.invalidRange'))
      return
    }
    setPending(true)
    setStatus('')
    try {
      const result = await client.enqueueBackfill(from.toISOString(), to.toISOString())
      const jobId = result?.job_id
      if (typeof jobId !== 'number') {
        throw new Error('missing job_id')
      }
      setStatus(
        result?.deduped ? t('settings.backfill.deduped', { id: jobId }) : t('settings.backfill.queued', { id: jobId })
      )
      pollJob(jobId)
    } catch {
      Message.error(t('settings.backfill.failed'))
      setPending(false)
    }
  }, [client, from, pollJob, t, to])

  return (
    <div className="mt-[12px] flex max-w-[520px] flex-col gap-2 py-1" data-testid="backfill-section">
      <div className="flex flex-col">
        <span className="text-[14px] font-bold text-[var(--color-text-1)]">{t('settings.backfill')}</span>
        <Text type="secondary" className="!text-[12px]">
          {t('settings.backfill.hint')}
        </Text>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <DatePicker
          value={from}
          onChange={(_, date) => date && setFrom(date.startOf('day'))}
          aria-label={t('settings.backfill.from')}
        />
        <span className="text-[12px] text-[var(--color-text-3)]">–</span>
        <DatePicker
          value={to}
          onChange={(_, date) => date && setTo(date.endOf('day'))}
          aria-label={t('settings.backfill.to')}
        />
        <Button type="secondary" className="mc-secondary-btn" loading={pending} onClick={() => void onSubmit()}>
          {t('settings.backfill.run')}
        </Button>
      </div>
      {status ? (
        <Text type="secondary" className="!text-[12px]" role="status">
          {status}
        </Text>
      ) : null}
    </div>
  )
}
