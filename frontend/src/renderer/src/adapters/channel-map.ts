// IPC 渠道 → HTTP 请求 的映射表。
//
// 这张表就是渠道契约的代码化形式。
// `fixtures/contract/used-ipc-channels.json` 列出了 preload 真正使用的渠道，
// 测试会确保每一个都被下面四个集合之一覆盖。

import type { HttpRequest } from './types.ts'

type RequestBuilder = (args: unknown[]) => HttpRequest

const enc = (value: unknown): string => encodeURIComponent(String(value ?? ''))

const query = (params: Record<string, unknown>): string => {
  const parts = Object.entries(params)
    .filter(([, value]) => value !== undefined && value !== null && value !== '')
    .map(([key, value]) => `${key}=${enc(value)}`)
  return parts.length > 0 ? `?${parts.join('&')}` : ''
}

const json = (args: unknown[], index = 0): unknown => args[index]

/**
 * 请求-响应式渠道。
 *
 * 注意参数顺序必须与 preload 一致 —— 例如 `getNewActivities(startTime, endTime)`，
 * 其中 endTime 的默认值是 `'2099-12-31 00:00:00'`（见 preload/index.ts）。
 */
export const CHANNEL_MAP: Record<string, RequestBuilder> = {
  // ---------------- database ----------------
  'database:get-all-activities': () => ({ method: 'GET', path: '/api/db/activities' }),
  'database:get-new-activities': (args) => ({
    method: 'GET',
    path: `/api/db/activities${query({
      start: args[0],
      end: args[1] ?? '2099-12-31 00:00:00'
    })}`
  }),
  'database:get-latest-activity': () => ({ method: 'GET', path: '/api/db/activities/latest' }),

  'database:get-all-vaults': () => ({ method: 'GET', path: '/api/db/vaults' }),
  'database:get-vaults-by-parent-id': (args) => ({
    method: 'GET',
    path: `/api/db/vaults${query({ parent_id: args[0] })}`
  }),
  'database:get-vault-by-id': (args) => ({
    method: 'GET',
    path: `/api/db/vaults/${enc(args[0])}`
  }),
  'database:get-vault-by-title': (args) => ({
    method: 'GET',
    path: `/api/db/vaults${query({ title: args[0] })}`
  }),
  'database:get-folders': () => ({ method: 'GET', path: '/api/db/vaults?is_folder=1' }),
  'database:get-vaults-by-document-type': (args) => ({
    method: 'GET',
    path: `/api/db/vaults${query({ document_type: args[0] })}`
  }),
  'database:insert-vault': (args) => ({
    method: 'POST',
    path: '/api/db/vaults',
    body: json(args)
  }),
  'database:update-vault-by-id': (args) => ({
    method: 'PATCH',
    path: `/api/db/vaults/${enc(args[0])}`,
    body: json(args, 1)
  }),
  'database:delete-vault-by-id': (args) => ({
    method: 'DELETE',
    path: `/api/db/vaults/${enc(args[0])}`
  }),
  'database:soft-delete-vault-by-id': (args) => ({
    method: 'POST',
    path: `/api/db/vaults/${enc(args[0])}/soft-delete`
  }),
  'database:restore-vault-by-id': (args) => ({
    method: 'POST',
    path: `/api/db/vaults/${enc(args[0])}/restore`
  }),
  'database:hard-delete-vault-by-id': (args) => ({
    method: 'DELETE',
    path: `/api/db/vaults/${enc(args[0])}/hard`
  }),
  'database:create-folder': (args) => ({
    method: 'POST',
    path: '/api/db/vaults/folders',
    body: { title: args[0], parent_id: args[1] ?? null }
  }),

  'database:get-all-tasks': (args) => ({
    method: 'GET',
    path: `/api/db/todos${query({ start: args[0], end: args[1] })}`
  }),
  'database:add-task': (args) => ({
    method: 'POST',
    path: '/api/db/todos',
    body: json(args)
  }),
  'database:update-task': (args) => ({
    method: 'PATCH',
    path: `/api/db/todos/${enc(args[0])}`,
    body: json(args, 1)
  }),
  'database:delete-task': (args) => ({
    method: 'DELETE',
    path: `/api/db/todos/${enc(args[0])}`
  }),
  'database:toggle-task-status': (args) => ({
    method: 'POST',
    path: `/api/db/todos/${enc(args[0])}/toggle`
  }),
  'database:get-all-tips': () => ({ method: 'GET', path: '/api/db/tips' }),

  'heatmap:get-data': (args) => ({
    method: 'GET',
    path: `/api/db/heatmap${query({ start: args[0], end: args[1] })}`
  }),

  // ---------------- capture / recording ----------------
  'screen-monitor:check-permissions': () => ({
    method: 'GET',
    path: '/api/capture/permissions'
  }),
  // 由 daemon 进程 Request：系统设置必须出现采集进程条目，只开外壳深链不够。
  'screen-monitor:open-prefs': () => ({
    method: 'POST',
    path: '/api/capture/permissions/request'
  }),
  'screen-monitor:get-visible-sources': () => ({
    method: 'GET',
    path: '/api/capture/targets?visible=1'
  }),
  'screen-monitor:get-capture-all-sources': () => ({
    method: 'GET',
    path: '/api/capture/targets'
  }),
  'screen-monitor:take-screenshot': (args) => ({
    // 第一个参数是分组时间，按它写；
    method: 'POST',
    path: '/api/capture/now',
    body: { group_interval: args[0], target_id: args[1] }
  }),
  'screen-monitor:delete-screenshot': (args) => ({
    method: 'DELETE',
    path: `/api/capture/screenshots${query({ path: args[0] })}`
  }),
  'screen-monitor:read-image-base64': (args) => ({
    method: 'GET',
    path: `/api/capture/screenshots/data${query({ path: args[0] })}`
  }),
  'screen-monitor:get-screenshots-by-date': (args) => ({
    method: 'GET',
    path: `/api/capture/screenshots${query({ date: args[0] })}`
  }),
  'screen-monitor:get-recording-stats': () => ({
    method: 'GET',
    path: '/api/monitoring/recording-stats'
  }),
  'screen-monitor:get-settings': (args) => ({
    method: 'GET',
    path: `/api/settings/${enc(args[0])}`
  }),
  'screen-monitor:set-settings': (args) => ({
    method: 'PUT',
    path: `/api/settings/${enc(args[0])}`,
    body: { value: args[1] }
  }),
  'screen-monitor:clear-settings': (args) => ({
    method: 'DELETE',
    path: `/api/settings/${enc(args[0])}`
  }),

  'task:start': () => ({ method: 'POST', path: '/api/capture/start' }),
  'task:stop': () => ({ method: 'POST', path: '/api/capture/stop' }),
  'task:check-can-record': () => ({ method: 'GET', path: '/api/capture/status' }),
  'task:update-model-config': (args) => ({
    method: 'PATCH',
    path: '/api/capture/config',
    body: json(args)
  }),
  'task:update-current-record-app': (args) => ({
    method: 'POST',
    path: '/api/capture/targets/selection',
    body: json(args)
  }),

  'home:get-latest-activity': (args) => ({
    method: 'POST',
    path: '/api/v1/latest-activity/poll',
    body: { state: args[0] }
  }),

  // ---------------- files ----------------
  'file:save': (args) => ({
    method: 'POST',
    path: '/api/files',
    body: { name: args[0], data: args[1] }
  }),
  'file:read': (args) => ({
    method: 'GET',
    path: `/api/files/${enc(args[0])}/data${args[1] === 'base64' ? '?encoding=base64' : ''}`
  }),
  'file:copy': (args) => ({
    method: 'POST',
    path: '/api/files/copy',
    body: { path: args[0] }
  }),
  'file:get-all': () => ({ method: 'GET', path: '/api/files' })
}

/**
 * **新增面**渠道：不在 preload 的既有契约里，前端直接调用（daemon 提供）。
 *
 * 单独成集的原因：`channel-map.test.ts` 会拿「preload 里真正用到的渠道」
 * 校验映射表有没有多余条目 —— 新增面本来就不在 preload 里，
 * 混进 `CHANNEL_MAP` 会被误判成多余。
 */
export const NEW_API_CHANNELS: Record<string, RequestBuilder> = {
  // 启动握手：渲染层拿不到 running 就停在加载页进不了主界面
  'backend:get-status': () => ({ method: 'GET', path: '/api/backend/status' }),
  // 总结卡片（新增面）
  'v1:summaries': () => ({ method: 'GET', path: '/api/v1/summaries' }),
  // 会话列表（多会话切换用）：走兼容路径
  'v1:conversations': (args) => ({
    method: 'GET',
    path: `/api/agent/chat/conversations/list${query({ limit: args[0] ?? 20 })}`
  }),
  // 某个会话的历史消息（切换会话时加载）
  'v1:conversation-messages': (args) => ({
    method: 'GET',
    path: `/api/agent/chat/conversations/${enc(args[0])}/messages`
  }),
  // 搜索页：关键词 + 时间窗；被拦截内容搜不到由服务端保证
  'v1:search': (args) => ({
    method: 'GET',
    path: `/api/v1/search${query({ q: args[0], start: args[1], end: args[2] })}`
  }),
  // 活动扩展面：带 origin / confidence / evidence（兼容面只有形状，没有来源）
  'v1:activities': () => ({ method: 'GET', path: '/api/v1/activities' }),
  // 用户改名：服务端先落事件再重算，前端只负责发请求与刷新
  'v1:activity-override': (args) => ({
    method: 'POST',
    path: '/api/v1/activities/overrides',
    body: { kind: 'rename', activity_id: args[0], title: args[1] }
  }),
  // 拆分：把一条活动在某时刻切成两段（`at` 是毫秒时间戳）
  'v1:activity-split': (args) => ({
    method: 'POST',
    path: '/api/v1/activities/overrides',
    body: { kind: 'split', activity_id: args[0], at: args[1], tail_title: args[2] }
  }),
  // 合并：把若干活动并进一个（同样是「先落事件再重算」）
  'v1:activity-merge': (args) => ({
    method: 'POST',
    path: '/api/v1/activities/overrides',
    body: { kind: 'merge', primary: args[0], absorbed: args[1] }
  }),
  // 任意时段总结：先预览范围与用量，再提交异步作业看进度
  'v1:adhoc-preview': (args) => ({
    method: 'GET',
    path: `/api/v1/summaries/adhoc/preview${query({ from: args[0], to: args[1] })}`
  }),
  'v1:adhoc-job-submit': (args) => ({
    method: 'POST',
    path: '/api/v1/summaries/adhoc/jobs',
    body: { from: args[0], to: args[1] }
  }),
  'v1:adhoc-job': (args) => ({
    method: 'GET',
    path: `/api/v1/summaries/adhoc/jobs/${enc(args[0])}`
  }),
  'v1:adhoc-job-cancel': (args) => ({
    method: 'POST',
    path: `/api/v1/summaries/adhoc/jobs/${enc(args[0])}/cancel`
  }),
  // 补偿推断作业：入队 + 查状态（设置页「补推断」入口）
  'v1:jobs-backfill': (args) => ({
    method: 'POST',
    path: '/api/v1/jobs/backfill',
    body: { from: args[0], to: args[1] }
  }),
  'v1:jobs-status': (args) => ({
    method: 'GET',
    path: `/api/v1/jobs/${enc(args[0])}`
  })
}

/**
 * 由 daemon 通过 SSE 推送的事件。
 * 键是既有 IPC 渠道名，值是 SSE 的 `event:` 名。
 */
export const SUBSCRIBED_CHANNELS: Record<string, string> = {
  'push:get-init-check-data': 'push:init-check-data',
  'push:power-monitor': 'push:power-monitor',
  'push:screen-monitor-status': 'push:screen-monitor-status',
  'push:latest-activity': 'push:latest-activity',
  // 后台总结作业的分块进度：完成后弹一条系统通知（用户可能在别的页面）
  'summary:progress': 'summary:progress'
}

/**
 * 由桌面外壳（Tauri）提供、不走 daemon HTTP 的能力。
 * 单独列出是为了防止有人误以为「没映射 = 漏了」。
 */
export const SHELL_CHANNELS: Record<string, string> = {
  'app:check-for-update': 'Tauri updater 插件',
  'app:quit-and-install': 'Tauri updater 插件',
  'app:cancel-download': 'Tauri updater 插件',
  'notification:send': 'Tauri notification 插件',
  'window-show': 'Tauri 窗口事件',
  'app-activate': 'Tauri 应用激活事件',
  'main-log': '渲染层日志输出，无需 IPC',
  'tray:update-recording-status': 'Tauri 命令 tray_recording_status（更新托盘提示与菜单文案）'
}

/**
 * 由外壳**推送**的渠道（不是 daemon 的 SSE）。
 *
 * 来源必须写清楚：订阅它们要走外壳事件（Tauri 的 `event.listen`），走 daemon
 * 只会得到一个永远不触发的监听器。
 */
export const SHELL_PUSH_CHANNELS: Record<string, string> = {
  'push:tray-toggle-recording': 'Tauri 托盘菜单「开始 / 暂停录制」发出',
  'push:tray-navigate-to-screen-monitor': 'Tauri 托盘菜单「屏幕监控」发出'
}

/**
 * 尚未实现、且已明确说明原因的渠道。
 * 每一条都必须有原因，测试会检查原因不是敷衍的空字符串。
 */
export const DEFERRED_CHANNELS: Record<string, string> = {
  'app:log-to-main': '日志改由 daemon 统一收集（--log），不再需要前端上报',
  'store-sync:subscribe': '多窗口状态同步；当前产品是单窗口，故为 no-op',
  'store-sync:unsubscribe': '多窗口状态同步；当前产品是单窗口，故为 no-op',
  'store-sync:on-update': '多窗口状态同步；当前产品是单窗口，故为 no-op',
  'store-sync:broadcast-sync': '跨窗口广播同上：单窗口产品没有第二个窗口可同步',
  'backend:status-changed': 'daemon 不推该事件；渲染层每 3 秒轮询 backend:get-status，够用'
}

export function resolveChannel(channel: string, args: unknown[]): HttpRequest | undefined {
  // 两张表都要查：`NEW_API_CHANNELS` 只是「不在 preload 契约里」的分组，
  // 它们同样要被解析 —— 漏掉这一半会让新增面在运行时报「渠道未映射」，
  // 而单测只看 CHANNEL_MAP 时仍然是绿的。
  const builder = CHANNEL_MAP[channel] ?? NEW_API_CHANNELS[channel]
  if (!builder) return undefined
  return builder(args)
}

/** 该渠道是否由 SSE 提供。 */
export function subscriptionEvent(channel: string): string | undefined {
  return SUBSCRIBED_CHANNELS[channel]
}
