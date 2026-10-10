// 助手引用 → 可跳转路径。
//
// 后端检索引用带稳定 `document_id` + `kind`（活动 / 笔记 / 总结）。
// 这里只做「种别与 id 形状 → 前端路由」的纯映射，渲染层负责真正导航。

export interface CitationSource {
  document_id: string
  title: string
  kind: string
  /** 文档时间（毫秒）；活动用来切到对应日期 */
  at?: number
}

/** 无法跳转时返回 null（未知 kind、笔记 id 形状不对等）。 */
export function citationPath(source: CitationSource): string | null {
  const kind = source.kind
  if (kind === 'document' || kind === 'note') {
    const match = /^note-(\d+)$/.exec(source.document_id)
    if (!match) return null
    return `/vault?id=${match[1]}`
  }
  if (kind === 'activity') {
    const params = new URLSearchParams({ activity: source.document_id })
    if (typeof source.at === 'number' && Number.isFinite(source.at)) {
      params.set('at', String(Math.trunc(source.at)))
    }
    return `/screen-monitor?${params.toString()}`
  }
  if (kind === 'summary') {
    return `/summaries?id=${encodeURIComponent(source.document_id)}`
  }
  return null
}
