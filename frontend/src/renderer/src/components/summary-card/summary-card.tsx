// 总结卡片：展示 `/api/v1/summaries` 的一条总结。
//
// 交付上要紧的是**质量标记**：`quality=model` 是模型写的，`quality=fallback`
// 是模板兜底。两者混在一起显示，用户就分不清「这条总结值不值得信」——
// 而兜底恰恰是模型不可用时悄悄发生的 —— 所以必须显式标出来。

import { useI18n } from '@renderer/i18n'
import { FC, useEffect, useState } from 'react'

interface SummaryRow {
  id: string
  title?: string
  /** daemon 的字段名是 `body_markdown`；另一种来源叫 `content`，两个都认。 */
  body_markdown?: string
  content?: string
  quality?: string
  /** RFC3339（daemon）或 `YYYY-MM-DD HH:MM:SS`，两种都认。 */
  start?: string
  end?: string
  start_time?: string
  end_time?: string
  evidence_count?: number
}

/** 只取 `HH:mm`：总结卡片上不需要完整日期。 */
function clock(value?: string): string {
  if (!value) return ''
  const match = value.match(/(\d{2}:\d{2})/)
  return match ? match[1] : value
}

export const SummaryCard: FC = () => {
  const { t } = useI18n()
  const [summaries, setSummaries] = useState<SummaryRow[] | null>(null)

  useEffect(() => {
    let alive = true
    void (async () => {
      try {
        const rows = await (
          window as unknown as {
            summaryApi?: { list?: () => Promise<SummaryRow[] | { summaries?: SummaryRow[] }> }
          }
        ).summaryApi?.list?.()
        // 容忍两种形状（裸数组 / `{ summaries }`）：适配层已经解包，这里再兜一层，
        // 避免任何一条路径漏解包时把整页打崩（对象上没有 `.map`，会直接抛）。
        const list = Array.isArray(rows) ? rows : (rows?.summaries ?? [])
        if (alive) setSummaries(list)
      } catch {
        // 后端不可用时也要有可读的空态，而不是空白卡片
        if (alive) setSummaries([])
      }
    })()
    return () => {
      alive = false
    }
  }, [])

  if (summaries === null) {
    return (
      <div className="flex items-center justify-center rounded-2xl border border-dashed border-[var(--color-border-2)] bg-[var(--color-bg-2)]/60 py-16 text-[13px] text-[var(--color-text-3)]">
        {t('summary.loading')}
      </div>
    )
  }

  if (summaries.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center gap-2 rounded-2xl border border-dashed border-[var(--color-border-2)] bg-[var(--color-bg-2)]/60 py-16">
        <p className="text-[13px] text-[var(--color-text-3)]">{t('summary.empty')}</p>
      </div>
    )
  }

  return (
    <div className="summary-card flex flex-col gap-3">
      {summaries.map((summary) => {
        const isFallback = (summary.quality ?? '') !== 'model'
        return (
          <article
            key={summary.id}
            className="rounded-2xl border border-[var(--color-border-2)] bg-[var(--color-bg-2)] p-5 transition-colors hover:border-[var(--color-border-2)]">
            <header className="mb-2 flex items-center justify-between gap-3">
              <h3 className="text-[15px] font-medium text-[var(--color-text-1)]">{summary.title}</h3>
              <span
                data-testid="summary-quality"
                className={`summary-quality--${isFallback ? 'fallback' : 'model'} shrink-0 rounded-[4px] px-1.5 py-0.5 text-xs ${isFallback ? 'bg-[var(--color-warning-light-1)] text-[rgb(var(--warning-6))]' : 'bg-[var(--color-success-light-1)] text-[rgb(var(--success-6))]'}`}>
                {isFallback ? t('summary.quality.fallback') : t('summary.quality.model')}
              </span>
            </header>
            <p className="mb-3 text-xs text-[var(--color-text-3)]">
              {clock(summary.start ?? summary.start_time)} – {clock(summary.end ?? summary.end_time)}
              {typeof summary.evidence_count === 'number' ? (
                <span> · {t('summary.evidenceCount', { count: summary.evidence_count })}</span>
              ) : null}
            </p>
            <div className="whitespace-pre-wrap text-[13px] leading-6 text-[var(--color-text-2)]">
              {summary.body_markdown ?? summary.content}
            </div>
          </article>
        )
      })}
    </div>
  )
}
