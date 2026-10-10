// 时间线的「这条结论怎么来的」：取扩展面（`/api/v1/activities`）的来源与置信度，
// 并提供用户改名入口。
//
// 为什么单独一次请求、而不是换掉时间线的数据源：兼容面 `/api/db/activities*`
// 的字段名与形状被 `pages/screen-monitor` 直接消费，换掉等于把整页重写；
// 扩展面只需要按 `legacy_id` 对上号即可。
//
// 拿不到扩展面（外壳渠道不可用 / 网络失败）时**不渲染标记**，而不是猜一个来源：
// 标错来源比不标更糟。

import { getLocale, translate } from '@renderer/i18n'
import { useCallback, useEffect, useState } from 'react'

/** 与 daemon 的 `Provenance` 一一对应（internally tagged）。 */
export interface ActivityOrigin {
  kind: 'observed' | 'rule' | 'inferred'
  rule_id?: string
  model?: string
}

export interface ActivityOriginRow {
  id: string
  legacy_id: number
  origin: ActivityOrigin
  /** 投影分类（开发 / 需求 / …）；分组时间线用它，没有就不分组到未分类 */
  category?: string | null
  /** 0–1；模型推断与规则命中才有意义 */
  confidence?: number
  is_user_modified?: boolean
}

export interface ProvenanceBadge {
  /** 来源类型：界面据它决定要不要给改名入口，**不要**去比展示字符串 */
  kind: ActivityOrigin['kind']
  /** 界面上的短标记（含置信度百分比，去掉无意义的 100%） */
  label: string
  /** 悬停时解释「为什么是这条结论」 */
  detail: string
}

interface ActivityApi {
  list?: () => Promise<unknown>
  rename?: (activityId: string, title: string) => Promise<unknown>
  merge?: (primaryId: string, absorbed: string[]) => Promise<unknown>
  split?: (activityId: string, atMs: number, tailTitle: string) => Promise<unknown>
}

const ORIGIN_LABEL_KEYS: Record<ActivityOrigin['kind'], string> = {
  observed: 'screenMonitor.origin.observed',
  rule: 'screenMonitor.origin.rule',
  inferred: 'screenMonitor.origin.inferred'
}

function activityApi(): ActivityApi | undefined {
  return (window as unknown as { activityApi?: ActivityApi }).activityApi
}

/** 把一行扩展面数据折成界面要的标记。 */
export function badgeOf(row: ActivityOriginRow): ProvenanceBadge {
  // 取词发生在调用时（badgeOf 不是组件）：语言切换后由上层重渲染拿到新文案
  const locale = getLocale()
  const kind = row.origin?.kind ?? 'observed'
  const base = translate(locale, ORIGIN_LABEL_KEYS[kind] ?? ORIGIN_LABEL_KEYS.observed)
  // 置信度只在非确定性来源上显示：规则命中与纯观察显示「100%」是噪音
  const percent =
    typeof row.confidence === 'number' && kind !== 'observed' ? ` ${Math.round(row.confidence * 100)}%` : ''
  let detail = translate(locale, 'screenMonitor.origin.observedDetail')
  if (kind === 'rule') {
    const ruleId = row.origin.rule_id
    detail = ruleId
      ? translate(locale, 'screenMonitor.origin.ruleDetailWithId', { ruleId })
      : translate(locale, 'screenMonitor.origin.ruleDetail')
  } else if (kind === 'inferred') {
    const model = row.origin.model
    detail = model
      ? translate(locale, 'screenMonitor.origin.inferredDetailWithModel', { model })
      : translate(locale, 'screenMonitor.origin.inferredDetail')
  }
  return { kind, label: `${base}${percent}`, detail }
}

export interface ActivityProvenance {
  /** 兼容层的数字 id → 来源行（v1 的字符串 id 也在里面） */
  rows: Map<string, ActivityOriginRow>
  badgeFor(legacyId: string | number): ProvenanceBadge | null
  /** 扩展面分类；拿不到时返回 null，调用方再退回 metadata / 未分类 */
  categoryFor(legacyId: string | number): string | null
  rename(legacyId: string | number, title: string): Promise<void>
  /** 把 `absorbedLegacyId` 并进 `primaryLegacyId`（两条都要能在扩展面里对上号） */
  merge(primaryLegacyId: string | number, absorbedLegacyId: string | number): Promise<void>
  /** 把某条活动在 `atMs` 处切成两段，后半段用 `tailTitle` */
  split(legacyId: string | number, atMs: number, tailTitle: string): Promise<void>
}

export function useActivityProvenance(): ActivityProvenance {
  const [rows, setRows] = useState<Map<string, ActivityOriginRow>>(() => new Map())

  const refresh = useCallback(async () => {
    const api = activityApi()
    if (!api?.list) return
    try {
      const payload = (await api.list()) as { activities?: ActivityOriginRow[] } | undefined
      const next = new Map<string, ActivityOriginRow>()
      for (const row of payload?.activities ?? []) {
        if (row && row.id) next.set(String(row.legacy_id ?? row.id), row)
      }
      setRows(next)
    } catch {
      // 扩展面拿不到就不显示标记：这里不该把整个时间线拖崩
      setRows(new Map())
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const badgeFor = useCallback(
    (legacyId: string | number): ProvenanceBadge | null => {
      const row = rows.get(String(legacyId))
      return row ? badgeOf(row) : null
    },
    [rows]
  )

  const categoryFor = useCallback(
    (legacyId: string | number): string | null => {
      const row = rows.get(String(legacyId))
      const category = row?.category?.trim()
      return category ? category : null
    },
    [rows]
  )

  const rename = useCallback(
    async (legacyId: string | number, title: string) => {
      const api = activityApi()
      const row = rows.get(String(legacyId))
      if (!api?.rename || !row) return
      // 发的是 **v1 的字符串 id**：接口按活动 id 找行，兼容层的数字 id 找不到
      await api.rename(row.id, title)
      await refresh()
    },
    [refresh, rows]
  )

  const merge = useCallback(
    async (primaryLegacyId: string | number, absorbedLegacyId: string | number) => {
      const api = activityApi()
      const primary = rows.get(String(primaryLegacyId))
      const absorbed = rows.get(String(absorbedLegacyId))
      if (!api?.merge || !primary || !absorbed) return
      // 发的是 **v1 的字符串 id** 数组：接口按活动 id 找行
      await api.merge(primary.id, [absorbed.id])
      await refresh()
    },
    [refresh, rows]
  )

  const split = useCallback(
    async (legacyId: string | number, atMs: number, tailTitle: string) => {
      const api = activityApi()
      const row = rows.get(String(legacyId))
      if (!api?.split || !row) return
      await api.split(row.id, atMs, tailTitle)
      await refresh()
    },
    [refresh, rows]
  )

  return { rows, badgeFor, categoryFor, rename, merge, split }
}
