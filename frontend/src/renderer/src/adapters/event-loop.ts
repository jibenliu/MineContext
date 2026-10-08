// `window.eventLoop`：首页「最新活动」轮询的开关。

import type { Backend } from './types.ts'

export interface EventLoopApi {
  getHomeLatestActivity(status: 'running' | 'stopped'): Promise<unknown>
}

export function createEventLoop(backend: Backend): EventLoopApi {
  return {
    getHomeLatestActivity: (status) => backend.invoke('home:get-latest-activity', status)
  }
}
