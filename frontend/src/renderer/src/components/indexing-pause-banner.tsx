// 向量索引因上游 401/429 等暂停时，在设置 / 首页给出原因与一键恢复。

import { Button } from '@arco-design/web-react'
import { useI18n } from '@renderer/i18n'
import { useCallback, useEffect, useState } from 'react'
import { Link as RouterLink } from 'react-router-dom'

export interface IndexingPauseInfo {
  paused?: boolean
  code?: string
  message?: string
  component?: string
  action?: { target?: string; label?: string } | null
}

export interface IndexingApi {
  status: () => Promise<{ paused?: boolean; indexing_pause?: IndexingPauseInfo | null } | undefined>
  resume: () => Promise<{ resumed?: boolean } | undefined>
}

function createIndexingApi(): IndexingApi {
  const api = (globalThis as { indexingApi?: IndexingApi }).indexingApi
  if (!api) {
    return {
      status: async () => undefined,
      resume: async () => undefined
    }
  }
  return api
}

export function IndexingPauseBanner({ api, pollMs = 8000 }: { api?: IndexingApi; pollMs?: number }) {
  const { t } = useI18n()
  const [client] = useState(() => api ?? createIndexingApi())
  const [pause, setPause] = useState<IndexingPauseInfo | null>(null)
  const [busy, setBusy] = useState(false)

  const refresh = useCallback(async () => {
    try {
      const data = await client.status()
      const next = data?.indexing_pause
      if (data?.paused && next?.message) {
        setPause(next)
      } else {
        setPause(null)
      }
    } catch {
      // 轮询失败不打扰：设置页其它操作仍可用
    }
  }, [client])

  useEffect(() => {
    void refresh()
    const id = setInterval(() => {
      void refresh()
    }, pollMs)
    return () => clearInterval(id)
  }, [pollMs, refresh])

  const onResume = useCallback(async () => {
    setBusy(true)
    try {
      await client.resume()
      setPause(null)
      await refresh()
    } finally {
      setBusy(false)
    }
  }, [client, refresh])

  if (!pause?.message) {
    return null
  }

  return (
    <div
      className="mb-3 flex max-w-[720px] flex-wrap items-center gap-2 rounded-lg bg-[rgba(var(--warning-1),1)] px-3 py-2 text-xs leading-5 text-[rgb(var(--warning-6))]"
      data-testid="indexing-pause-banner"
      title={pause.code}>
      <span data-testid="indexing-pause-message">{pause.message}</span>
      <Button
        size="mini"
        type="primary"
        loading={busy}
        disabled={busy}
        data-testid="indexing-pause-resume"
        onClick={() => {
          void onResume()
        }}>
        {pause.action?.label || t('indexing.pause.resume')}
      </Button>
      <RouterLink
        to="/settings?section=model"
        className="text-xs leading-5 text-[rgb(var(--primary-6))] no-underline hover:underline"
        data-testid="indexing-pause-settings">
        {t('indexing.pause.openSettings')}
      </RouterLink>
    </div>
  )
}
