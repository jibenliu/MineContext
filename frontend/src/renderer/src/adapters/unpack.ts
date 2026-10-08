// 响应解包：把 daemon 的信封负载整理成消费方要的形状。
//
// 为什么要单独一层：`v1:summaries` 返回的是 `{ summaries: [...] }`，而消费方
// （总结卡片）只想拿到数组 —— 解包散落在调用点最容易漏掉，漏一处就是
// `summaries.map is not a function` 白屏。集中在这里之后：
//   1. 每个解包函数都是纯函数，可以拿 `fixtures/contract/response-shapes.json`
//      里**由 daemon 侧测试校验过的真实样例**直接测；
//   2. 同时容忍裸数组与 `{results}` —— 兼容期两边都能跑，真实形状由 fixture 钉住。

/** 总结列表：`{ summaries: [...] }`；也容忍裸数组。 */
export function unwrapSummaries(payload: unknown): unknown[] {
  if (Array.isArray(payload)) return payload
  const summaries = (payload as { summaries?: unknown } | null | undefined)?.summaries
  return Array.isArray(summaries) ? summaries : []
}

/** 对话列表：`{ items: [...], total: number }`。 */
export function unwrapConversations(payload: unknown): { items: unknown[]; total: number } {
  if (Array.isArray(payload)) return { items: payload, total: payload.length }
  const record = (payload ?? {}) as { items?: unknown; total?: unknown }
  const items = Array.isArray(record.items) ? record.items : []
  const total = typeof record.total === 'number' ? record.total : items.length
  return { items, total }
}

/** 检索结果：`{ query, results: [...] }`；也容忍裸数组。 */
export function unwrapSearchResults(payload: unknown): unknown[] {
  if (Array.isArray(payload)) return payload
  const results = (payload as { results?: unknown } | null | undefined)?.results
  return Array.isArray(results) ? results : []
}
