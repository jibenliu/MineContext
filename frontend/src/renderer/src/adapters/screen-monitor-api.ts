// `window.screenMonitorAPI` 的形状（与 preload 一致）。

import type { Backend } from './types.ts'

export interface ScreenMonitorApi {
  checkPermissions(): Promise<unknown>
  openPrefs(): Promise<unknown>
  takeScreenshot(groupIntervalTime: string, sourceId: string): Promise<unknown>
  getVisibleSources(): Promise<unknown>
  deleteScreenshot(filePath: string): Promise<unknown>
  readImageAsBase64(filePath: string): Promise<unknown>
  getScreenshotsByDate(date?: string): Promise<unknown>
  getCaptureAllSources(thumbnailSize?: unknown): Promise<unknown>
  getSettings<T = unknown>(key: string): Promise<T>
  setSettings(key: string, value: unknown): Promise<unknown>
  clearSettings(key: string): Promise<unknown>
  getRecordingStats(): Promise<unknown>
  updateModelConfig(config: unknown): Promise<unknown>
  startTask(): Promise<unknown>
  stopTask(): Promise<unknown>
  updateCurrentRecordApp(appInfo: unknown): Promise<unknown>
  checkCanRecord(): Promise<unknown>
}

export function createScreenMonitorApi(backend: Backend): ScreenMonitorApi {
  return {
    checkPermissions: () => backend.invoke('screen-monitor:check-permissions'),
    openPrefs: () => backend.invoke('screen-monitor:open-prefs'),
    takeScreenshot: (groupIntervalTime, sourceId) =>
      backend.invoke('screen-monitor:take-screenshot', groupIntervalTime, sourceId),
    getVisibleSources: () => backend.invoke('screen-monitor:get-visible-sources'),
    deleteScreenshot: (filePath) => backend.invoke('screen-monitor:delete-screenshot', filePath),
    readImageAsBase64: (filePath) => backend.invoke('screen-monitor:read-image-base64', filePath),
    getScreenshotsByDate: (date) => backend.invoke('screen-monitor:get-screenshots-by-date', date),
    // daemon 的 `/api/capture/targets` 直接返回目标数组，而调用方
    // 读的是 `{ success, sources }`。形状转换放在适配层 —— 那是它存在的意义，
    // 页面只关心目标数组，不关心它从哪来。
    getCaptureAllSources: async (thumbnailSize) => {
      const result = (await backend.invoke('screen-monitor:get-capture-all-sources', thumbnailSize)) as unknown
      if (Array.isArray(result)) {
        return { success: true, sources: result }
      }
      return result
    },
    getSettings: <T = unknown>(key: string) => backend.invoke<T>('screen-monitor:get-settings', key),
    setSettings: (key, value) => backend.invoke('screen-monitor:set-settings', key, value),
    clearSettings: (key) => backend.invoke('screen-monitor:clear-settings', key),
    getRecordingStats: () => backend.invoke('screen-monitor:get-recording-stats'),
    updateModelConfig: (config) => backend.invoke('task:update-model-config', config),
    startTask: () => backend.invoke('task:start'),
    stopTask: () => backend.invoke('task:stop'),
    updateCurrentRecordApp: (appInfo) => backend.invoke('task:update-current-record-app', appInfo),
    checkCanRecord: () => backend.invoke('task:check-can-record')
  }
}
