// F1：`resources` 是后端序列化的 JSON 字符串，解析失败不能把页面打崩。
//
// 为什么单独一层：入库的 JSON 可能来自任何写入方（导入、旧版本升级、字段演进），都可能让它
// 不是合法 JSON 或干脆缺失；解析写在渲染路径上时，一次 `SyntaxError` 就是整页白屏
// （`App.tsx` 的启动握手出过同类问题）。因此统一走这个容错入口。
//
// 参照实现是 `pages/home/components/chat-card/chat-card.tsx` 里对 `metadata` 的处理。

/** 解析后端给的 JSON 数组字段；坏数据 / 缺失 / 形状不对时返回空数组。 */
export function parseJsonArray(raw: unknown): unknown[] {
  if (Array.isArray(raw)) return raw
  if (typeof raw !== 'string' || raw.trim() === '') return []
  try {
    const parsed: unknown = JSON.parse(raw)
    return Array.isArray(parsed) ? parsed : []
  } catch {
    return []
  }
}

/**
 * 活动行：把 `resources` 从 JSON 字符串解成数组。
 *
 * 逐字段解包而**不是**整行丢弃：活动本身的信息（标题/时间/应用）仍然有用，
 * 一条坏 `resources` 不该让整条活动从时间线上消失。
 */
export function withParsedResources<T extends { resources?: unknown }>(row: T): T & { resources: unknown[] } {
  return { ...row, resources: parseJsonArray(row?.resources) }
}
