// 启动握手的判断：拿到 `push:init-check-data` 的负载后，该进引导页还是主界面。
//
// 为什么单独一个纯函数：原来的判断式写在 `App.tsx` 的回调里
// （`setShowSettings(!temp.data.components.llm)`），而 `llm` 在现行契约里**恒为对象**，
// 对象恒 truthy → `!llm` 恒 false → 未配置模型的用户被直接放进主界面，
// 采集与总结随后静默失败。抽出来才能用测试钉住语义（对象/字符串/坏数据三种输入）。

/** 引导页的判据。`raw` 是 SSE 推来的字符串（消费方自己解析的部分）。 */
export function shouldShowOnboarding(raw: unknown): boolean {
  const payload = parsePayload(raw)
  // 解析不了就按"需要引导"处理：宁可多显示一次设置页，也不要让未配置的用户
  // 进到主界面后什么都做不了（而且那条路径没有任何提示）。
  return payload?.data?.components?.llm?.status !== 'ok'
}

interface InitCheckPayload {
  data?: { components?: { llm?: { status?: string } } }
}

function parsePayload(raw: unknown): InitCheckPayload | undefined {
  if (raw === null || raw === undefined) return undefined
  if (typeof raw === 'object') return raw as InitCheckPayload
  if (typeof raw !== 'string') return undefined
  try {
    return JSON.parse(raw) as InitCheckPayload
  } catch {
    return undefined
  }
}
