// `window.serverPushAPI` 的形状：daemon → 渲染层的推送。
//
// 事件名沿用既有渠道名，因此消费方（events/atom/NotificationProvider）零改动。
//
// 负载约定**按渠道不同**，这里是唯一的解释处：
//   - `push:get-init-check-data`：消费方（`App.tsx`）拿到字符串后自己 `JSON.parse`
//     （推送负载本来就是字符串），因此这里**原样透传**；
//   - 其余渠道：消费方直接用值（`setLatestActivity(data)`、`status === 'running'`），
//     而 SSE 帧里是 JSON 文本，因此这里解成值；解不出来就原样给（不吞掉内容）。

import type { Backend } from './types.ts'

export interface ServerPushApi {
  getInitCheckData(callback: (data: unknown) => void): () => void
  powerMonitor(callback: (payload: unknown) => void): () => void
  pushScreenMonitorStatus(callback: (status: unknown) => void): () => void
  pushHomeLatestActivity(callback: (activity: unknown) => void): () => void
  /** 后台总结作业的分块进度（完成时用来发系统通知）。 */
  summaryProgress(callback: (frame: unknown) => void): () => void
}

/** SSE 帧的负载是 JSON 文本，消费方要的是值。 */
export function parsePushPayload(payload: unknown): unknown {
  if (typeof payload !== 'string') return payload
  try {
    return JSON.parse(payload)
  } catch {
    return payload
  }
}

export function createServerPushApi(backend: Backend): ServerPushApi {
  return {
    getInitCheckData: (callback) => backend.subscribe('push:get-init-check-data', callback),
    powerMonitor: (callback) =>
      backend.subscribe('push:power-monitor', (payload) => callback(parsePushPayload(payload))),
    pushScreenMonitorStatus: (callback) =>
      backend.subscribe('push:screen-monitor-status', (payload) => callback(parsePushPayload(payload))),
    pushHomeLatestActivity: (callback) =>
      backend.subscribe('push:latest-activity', (payload) => callback(parsePushPayload(payload))),
    summaryProgress: (callback) =>
      backend.subscribe('summary:progress', (payload) => callback(parsePushPayload(payload)))
  }
}
