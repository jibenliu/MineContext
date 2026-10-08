// 对话里的总结指令：命中就交给任意时段总结的作业接口，**不发给模型**。
//
// 抽成依赖注入的函数，是为了在不渲染整个对话界面的前提下也能测到这条判断：
// 「命中时不调模型」「没命中时不拦」「提交失败也不回退成普通提问」。

import { parseSummaryIntent } from './summary-intent'

export interface SummaryCommandDeps {
  submit(from: string, to: string): Promise<unknown>
  onDispatched(label: string, path: string): void
  onFailed(message: string): void
}

/**
 * 返回 true 表示这条消息已被当作指令处理（调用方**不要再发给模型**），
 * 返回 false 表示不是指令，按普通对话继续。
 */
export async function dispatchSummaryCommand(
  text: string,
  deps: SummaryCommandDeps,
  now?: Parameters<typeof parseSummaryIntent>[1]
): Promise<boolean> {
  const intent = parseSummaryIntent(text, now)
  if (!intent) return false

  try {
    const submitted = (await deps.submit(intent.from, intent.to)) as { job_id?: string } | undefined
    if (!submitted?.job_id) throw new Error('总结作业缺少 job_id')
    const params = new URLSearchParams({ job_id: submitted.job_id, from: intent.from, to: intent.to })
    deps.onDispatched(intent.label, `/summaries?${params}`)
  } catch (error) {
    // 提交失败也要算「已处理」：把这句话再丢给模型，用户会得到一段
    // 与本地数据无关的文字，比明说失败更糟
    deps.onFailed(`生成失败：${String(error)}`)
  }
  return true
}

/**
 * 对话侧的确认文案：必须说清**进度在哪里看** —— 作业在后台跑，界面若不提，
 * 用户会以为总结没开始（进度与取消在总结页）。
 */
export function dispatchedMessage(label: string): string {
  return `已开始生成「${label}」的总结 —— 进度与取消在总结页`
}
