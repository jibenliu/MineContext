// 搜索页。
//
// 服务端 `GET /api/v1/search` 已经支持关键词命中 + 时间过滤，并且与聊天流
// 走同一条检索路径（因此被拦截内容搜不到）。这一层只做两件事：
//   1. 把关键词与时间窗发给它；
//   2. 结果为空时给**可读的空态**（「没有匹配」与「还没搜」必须能区分开）——
//      两者长得一样时，用户会以为搜索坏了。

import { Button, Input } from '@arco-design/web-react'
import { ListPagination } from '@renderer/components/list-pagination'
import { useI18n } from '@renderer/i18n'
import { FC, useEffect, useRef, useState } from 'react'

interface SearchHit {
  id: string
  kind: string
  title: string
  snippet?: string
  score?: number
  at?: number
}
const PAGE_SIZE = 20

/**
 * 时间边界 → 毫秒时间戳（本地时区）。两种输入都认：
 * - `YYYY-MM-DD`（只给日期）：那一天的 00:00 / 23:59:59.999；
 * - `YYYY-MM-DDTHH:mm`（带了时刻）：**原样保留** —— 用户选了 30 分钟，
 *   折成整天会让他以为筛错了。
 */
export function rangeBound(value: string, edge: 'start' | 'end'): number | undefined {
  if (!value) return undefined
  const hasTime = value.includes('T')
  const normalized = hasTime ? value : `${value}T${edge === 'start' ? '00:00:00' : '23:59:59.999'}`
  const parsed = new Date(normalized).getTime()
  return Number.isNaN(parsed) ? undefined : parsed
}

export const SearchResults: FC = () => {
  const { t } = useI18n()
  const [text, setText] = useState('')
  // 时间窗：服务端支持 `start`/`end`，前端不暴露等于没有。
  // 表单用 `YYYY-MM-DD`（人写的），转成毫秒时间戳发给 daemon。
  const [from, setFrom] = useState('')
  const [to, setTo] = useState('')
  const [hits, setHits] = useState<SearchHit[] | null>(null)
  const [searched, setSearched] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [page, setPage] = useState(0)
  const requestRef = useRef(0)
  const pendingRef = useRef(false)

  useEffect(
    () => () => {
      requestRef.current += 1
    },
    []
  )

  const run = async () => {
    const query = text.trim()
    if (!query || pendingRef.current) return
    const start = rangeBound(from, 'start')
    const end = rangeBound(to, 'end')
    if (
      (from && start === undefined) ||
      (to && end === undefined) ||
      (start !== undefined && end !== undefined && start > end)
    ) {
      setError(t('search.invalidRange'))
      return
    }
    const request = ++requestRef.current
    pendingRef.current = true
    setBusy(true)
    setError(null)
    setHits(null)
    setPage(0)
    try {
      const api = (
        window as unknown as {
          searchApi?: {
            query: (q: string, start?: number, end?: number) => Promise<{ results?: SearchHit[] } | SearchHit[]>
          }
        }
      ).searchApi
      if (!api) throw new Error('Search unavailable')
      const result = await api.query(query, start, end)

      if (request !== requestRef.current) return
      const rows = Array.isArray(result) ? result : (result?.results ?? [])
      setHits(rows)
      setSearched(true)
    } catch {
      if (request === requestRef.current) setError(t('search.failed'))
    } finally {
      pendingRef.current = false
      if (request === requestRef.current) setBusy(false)
    }
  }

  const precision = from.includes('T') || to.includes('T')

  return (
    <div className="search-results flex flex-col gap-4">
      {/* 搜索条件区：白底圆角卡片，与其它页面卡片一致 */}
      <div className="flex flex-col gap-3 rounded-2xl border border-[var(--color-border-2)] bg-white p-4">
        {/* 关键词与时间窗排成两行：挤在一行时输入框会被压扁到看不见 */}
        <div className="flex items-center gap-2">
          <Input
            aria-label={t('search.keywordLabel')}
            placeholder={t('search.keyword.placeholder')}
            value={text}
            disabled={busy}
            onChange={setText}
            onKeyDown={(event) => {
              if (event.key === 'Enter') void run()
            }}
            className="!w-[320px]"
          />
          <Button type="primary" loading={busy} disabled={!text.trim()} onClick={() => void run()}>
            {t('search.action')}
          </Button>
        </div>
        <div className="flex flex-wrap items-end gap-3">
          <label className="flex flex-col gap-1 text-xs text-[var(--color-text-3)]">
            {t('search.from')}
            <Input
              aria-label={t('search.fromLabel')}
              type="datetime-local"
              value={from}
              disabled={busy}
              onChange={setFrom}
              className="!w-[200px]"
            />
          </label>
          <label className="flex flex-col gap-1 text-xs text-[var(--color-text-3)]">
            {t('search.to')}
            <Input
              aria-label={t('search.toLabel')}
              type="datetime-local"
              value={to}
              disabled={busy}
              onChange={setTo}
              className="!w-[200px]"
            />
          </label>
          <span className="pb-1 text-xs text-[var(--color-text-3)]">
            {precision ? t('search.rangeHint.precise') : t('search.rangeHint')}
          </span>
        </div>
      </div>

      {error && <p role="alert">{error}</p>}
      {busy ? (
        <p role="status">{t('search.loading')}</p>
      ) : error ? null : hits === null || hits.length === 0 ? (
        <div className="flex flex-col items-center justify-center gap-2 rounded-2xl border border-dashed border-[var(--color-border-2)] bg-white/60 py-16">
          <p className="text-[13px] text-[var(--color-text-3)]">
            {searched ? t('search.empty.noHits') : t('search.empty.notSearched')}
          </p>
        </div>
      ) : (
        <ul className="flex flex-col gap-2">
          {hits.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE).map((hit) => (
            <li
              key={hit.id}
              data-testid="search-hit"
              className="rounded-2xl border border-[var(--color-border-2)] bg-white px-4 py-3 transition-colors hover:border-[var(--color-border-2)] hover:bg-[var(--color-bg-1)]">
              <div className="flex items-center gap-2">
                <span className="text-[14px] font-medium text-[var(--color-text-1)]">{hit.title}</span>
                <span className="rounded-[4px] bg-[var(--color-fill-2)] px-1.5 py-0.5 text-[11px] text-[var(--color-text-3)]">
                  {hit.kind}
                </span>
              </div>
              {hit.snippet ? (
                <div className="mt-1.5 line-clamp-2 text-[12px] leading-5 text-[var(--color-text-3)]">
                  {hit.snippet}
                </div>
              ) : null}
            </li>
          ))}
        </ul>
      )}
      {!busy && !error && hits && (
        <ListPagination page={page} pages={Math.ceil(hits.length / PAGE_SIZE)} onChange={setPage} />
      )}
    </div>
  )
}
