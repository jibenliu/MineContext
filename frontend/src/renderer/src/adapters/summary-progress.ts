// 「后台作业完成」的判定与通知：daemon 通过 `summary:progress` 推每个分块的进度，
// 用户在别的页面时也应该知道结果好了。
//
// 判定与副作用分开：判定是纯函数（可测），通知走 `window.api.notification.send`
// （外壳没声明通知能力时它会明确抛错，这里只记录不弹窗、也不影响其他逻辑）。

export interface SummaryProgressFrame {
  job_id?: unknown
  chunks_done?: unknown
  chunks_total?: unknown
  progress?: unknown
}

/** 只认「已经全部完成」这一种终态；其余帧返回 undefined（不猜、不提前通知）。 */
export function completedJobId(frame: unknown): string | undefined {
  if (!frame || typeof frame !== 'object') return undefined
  const value = frame as SummaryProgressFrame
  if (typeof value.job_id !== 'string' || value.job_id.length === 0) return undefined
  if (typeof value.chunks_total !== 'number' || value.chunks_total <= 0) return undefined
  if (typeof value.chunks_done !== 'number') return undefined
  if (value.chunks_done < value.chunks_total) return undefined
  return value.job_id
}

export interface SummaryNotifier {
  send(notification: { title: string; message: string }): Promise<unknown>
}

/**
 * 订阅进度并在作业完成时通知一次。
 *
 * 返回取消订阅函数；通知失败（例如外壳没声明 notification 能力）只记录日志，
 * 因为「没弹通知」不该影响总结本身已经生成这个事实。
 */
export function watchSummaryProgress(
  subscribe: (handler: (frame: unknown) => void) => () => void,
  notify: ((notification: { title: string; message: string }) => Promise<unknown>) | undefined,
  onDone: (jobId: string) => void,
  log: (message: string, error?: unknown) => void = () => {}
): () => void {
  const notified = new Set<string>()
  return subscribe((frame) => {
    const jobId = completedJobId(frame)
    if (!jobId || notified.has(jobId)) return
    notified.add(jobId)
    onDone(jobId)
    if (!notify) {
      log('作业完成，但当前外壳没有通知能力，未弹系统通知')
      return
    }
    void notify({ title: '总结已生成', message: '后台总结已经完成，打开总结页查看。' }).catch((error) => {
      log('系统通知发送失败（不影响总结本身）', error)
    })
  })
}
