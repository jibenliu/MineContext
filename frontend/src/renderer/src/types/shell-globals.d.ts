// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0
// 渲染层依赖的全局形状。
//
// 这些名字（dbAPI / screenMonitorAPI / api …）是业务代码直接调用的，由适配层在
// 启动时装到 window 上（见 adapters/install.ts）。外壳是谁不影响它们 —— 外壳
// 只负责把 daemon 的端口与 token 交给渲染层。
import type { AppApi } from '@renderer/adapters/app-api'
import type { IpcRendererShim } from '@renderer/adapters/ipc-renderer-shim'

interface ScreenMonitorAPI {
  /** 可能是布尔，或 `/api/capture/permissions` 的结构体 */
  checkPermissions: () => Promise<
    | boolean
    | {
        screen_recording?: boolean
        screen_recording_tcc?: boolean
        windows_reason?: string | null
        permission?: string
        status?: string
        ready?: boolean
        enabled?: boolean
        running?: boolean
        message?: string
      }
  >
  openPrefs: () => Promise<void>
  takeScreenshot: (
    groupIntervalTime: string,
    sourceId: string
  ) => Promise<{
    success: boolean
    screenshotInfo?: { url: string; date: string; timestamp: number }
    error?: string
  }>
  getVisibleSources: (ids?: string[]) => Promise<{ success: boolean; sources?: any[]; error?: string }>
  deleteScreenshot: (filePath: string) => Promise<{ success: boolean; error?: string }>
  readImageAsBase64: (filePath: string) => Promise<{
    success: boolean
    data?: string
    mime?: string
    error?: string
  }>
  getScreenshotsByDate: (
    date?: string,
    recordInterval?: number
  ) => Promise<{
    success: boolean
    screenshots?: Array<{
      id: string
      date: string
      timestamp: number
      image_url: string
      description: string
      created_at: string
      group_id: string
    }>
    error?: string
  }>
  getCaptureAllSources: (thumbnailSize?: { width: number; height: number }) => Promise<{
    success: boolean
    sources?: any[]
    error?: string
  }>
  getSettings: <T>(key: string) => Promise<T>
  setSettings: (
    key: string,
    value: unknown
  ) => Promise<{
    success: boolean
    error?: string
  }>
  clearSettings: (key: string) => Promise<{
    success: boolean
    error?: string
  }>
  updateCurrentRecordApp: (appInfo: CaptureSource[]) => Promise<{
    success: boolean
    error?: string
  }>
  updateModelConfig: (config: ScreenSettings) => Promise<{
    success: boolean
    error?: string
  }>
  stopTask: () => Promise<{
    success: boolean
    error?: string
  }>
  startTask: () => Promise<{
    success: boolean
    error?: string
  }>
  checkCanRecord: () => Promise<{
    canRecord: boolean
    status: string
    /** capture.enabled：false 表示用户停过录，间隔配置不等于正在采集 */
    enabled?: boolean
    /** 进程 TCC 原值；与经验证后的 canRecord / screen_recording 区分 */
    screen_recording_tcc?: boolean
    /** 不能录制时的原因（后端 /api/capture/status 的 reason） */
    reason?: string
    windows_reason?: string | null
  }>
  getRecordingStats: () => Promise<{
    captured_screenshots: number
    processed_screenshots: number
    failed_screenshots: number
    generated_activities: number
    next_activity_eta_seconds: number
    last_activity_time: string | null
    session_start_time: string
    recent_errors: Array<{
      error_message: string
      processor_name: string
      timestamp: string
    }>
    recent_screenshots: string[]
  } | null>
}

interface dbAPI {
  getVaultsByDocumentType: (documentType: VaultDocumentType | VaultDocumentType[]) => Promise<Vault[]>
  getVaultByTitle: (title: string) => Promise<Vault[]>
  getAllVaults: () => Promise<Vault[]>
  getHeatmapData: (startTime: number, endTime: number) => Promise<HeatmapData[]>
  getTasks: (startTime: string, endTime: string) => Promise<TODOActivity[]>
  [propName: string]: (...args: any[]) => any
}
interface EventLoopAPI {
  getHomeLatestActivity: (status: string) => Promise<LatestActivity[]>
  [propName: string]: (...args: any[]) => any
}
interface serverPushAPI {
  pushHomeLatestActivity: (callback: (data: Activity) => void) => any
  [propName: string]: (...args: any[]) => any
}

interface LinkApi {
  importUrl: (
    url: string,
    parentId?: number | null
  ) => Promise<{ id: number; title: string; url: string; source_host: string }>
}

interface ContextSourceApi {
  importRss: (
    url: string,
    parentId?: number | null,
    limit?: number | null
  ) => Promise<{
    feed_title: string
    feed_url: string
    source_host: string
    imported: Array<{ id: number; title: string; link: string }>
    skipped: number
  }>
  importResearch: (
    topic: string,
    urls: string[],
    parentId?: number | null
  ) => Promise<{ id: number; title: string; sources: string[] }>
  importFolder: (
    path: string,
    parentId?: number | null,
    recursive?: boolean
  ) => Promise<{
    path: string
    imported: Array<{ id: number; title: string; name: string; kind: string; file_path: string }>
    skipped: number
    errors: string[]
  }>
  trackFolder: (path: string) => Promise<{ path: string }>
  listTrackedFolders: () => Promise<{ folders: Array<{ path: string }> }>
  syncTrackedFolders: (
    path?: string | null,
    parentId?: number | null
  ) => Promise<{
    path: string
    imported: Array<{ id: number; title: string; name: string; kind: string; file_path: string }>
    skipped: number
    errors: string[]
  }>
}

declare global {
  interface Window {
    /**
     * 适配层提供的 `ipcRenderer` 形状：把外壳渠道映射到 Tauri 能力
     * （`adapters/ipc-renderer-shim.ts`），屏幕监控等调用点按老写法使用。
     */
    electron: { ipcRenderer: IpcRendererShim }
    api: AppApi
    dbAPI: dbAPI
    screenMonitorAPI: ScreenMonitorAPI
    fileService: any
    serverPushAPI: serverPushAPI
    eventLoop: EventLoopAPI
    linkApi: LinkApi
    contextSourceApi: ContextSourceApi
  }
}
