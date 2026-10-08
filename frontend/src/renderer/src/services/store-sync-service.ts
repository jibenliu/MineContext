// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import type { Middleware, Store, UnknownAction } from '@reduxjs/toolkit'
import { IpcChannel } from '@shared/ipc-channel'
import { getLogger } from '@shared/logger/renderer'
import type { StoreSyncAction } from '@types'

const logger = getLogger('StoreSyncService')

export type SyncOptions = {
  /** Whitelist of action type prefixes to sync, e.g., ['user/', 'settings/'] */
  syncList: string[]
  /** Optional: Finer-grained filtering logic (returns true to participate in sync) */
  shouldSync?: (action: UnknownAction) => boolean
}

function isFSA(action: any): action is { type: string; [k: string]: any } {
  return action && typeof action === 'object' && typeof action.type === 'string'
}

export class StoreSyncService {
  private static instance: StoreSyncService | null = null
  static getInstance() {
    if (!this.instance) this.instance = new StoreSyncService()
    return this.instance
  }

  private store: Store | null = null
  private options: SyncOptions = { syncList: [] }
  private broadcastSyncRemover: (() => void) | null = null
  private initialized = false

  /**
   * Unified initialization entry point: injects store + options, and automatically completes subscription
   */
  init(store: Store, options?: Partial<SyncOptions>) {
    if (this.initialized) {
      logger.warn('StoreSyncService already initialized; ignoring subsequent init.')
      return
    }

    this.store = store
    this.options = { ...this.options, ...(options || {}) }

    this.subscribe() // Automatic subscription

    // Automatic cleanup
    window.addEventListener('beforeunload', () => this.unsubscribe())
    this.initialized = true
  }

  /** Redux middleware: intercepts and broadcasts whitelisted local actions */
  createMiddleware(): Middleware {
    return () => (next) => (action) => {
      const result = next(action)

      if (!isFSA(action)) return result

      const isFromSync = Boolean((action as StoreSyncAction)?.meta?.fromSync)
      const inWhitelist = this.shouldSyncAction(action.type)
      const passCustom = this.options.shouldSync ? this.options.shouldSync(action) : true

      if (!isFromSync && inWhitelist && passCustom) {
        try {
          // 通知外壳广播到其它窗口
          window.api?.storeSync?.onUpdate(action as StoreSyncAction)
        } catch (e) {
          logger.error('storeSync.onUpdate failed:', e as Error)
        }
      }

      return result
    }
  }

  /** 适配层未就绪时的重试定时器。 */
  private retryTimer: number | null = null

  /** Whitelist matching (prefix) */
  private shouldSyncAction(actionType: string): boolean {
    const { syncList } = this.options
    if (!Array.isArray(syncList) || syncList.length === 0) return false
    return syncList.some((prefix) => actionType.startsWith(prefix))
  }

  /** 订阅外壳广播（私有） */
  private subscribe() {
    if (this.broadcastSyncRemover) return
    if (!window.api?.storeSync) {
      // 这个服务在模块求值阶段就订阅，而 `window.api` 由适配层在
      // `bootstrapBackend` 里安装 —— 早于它调用时等一次宏任务再来，
      // 而不是直接放弃（多窗口同步在单窗口产品里是可选项，但「静默关掉」
      // 会让人以为同步坏了）。
      if (this.retryTimer === null) {
        this.retryTimer = window.setTimeout(() => {
          this.retryTimer = null
          this.subscribe()
        }, 200)
      }
      return
    }

    // 监听来自外壳的动作广播
    this.broadcastSyncRemover = window.electron.ipcRenderer.on(
      IpcChannel.StoreSync_BroadcastSync,
      (_evt, action: StoreSyncAction) => {
        try {
          if (!this.store) return
          // Mark fromSync to prevent loops
          const synced: StoreSyncAction = {
            ...action,
            meta: { ...(action.meta || {}), fromSync: true }
          }
          this.store.dispatch(synced as unknown as UnknownAction)
        } catch (error) {
          logger.error('Error dispatching synced action:', error as Error)
        }
      }
    )

    // 发起订阅（让外壳把本窗口加入广播名单）
    try {
      window.api.storeSync.subscribe()
    } catch (e) {
      logger.error('storeSync.subscribe failed:', e as Error)
    }
  }

  /** Unsubscribe (private) */
  private unsubscribe() {
    if (this.retryTimer !== null) {
      window.clearTimeout(this.retryTimer)
      this.retryTimer = null
    }
    try {
      window.api?.storeSync?.unsubscribe()
    } catch (e) {
      logger.error('storeSync.unsubscribe failed:', e as Error)
    }

    if (this.broadcastSyncRemover) {
      try {
        this.broadcastSyncRemover()
      } catch (e) {
        // Some preload wrappers may not be function removers, so swallow the error here
      } finally {
        this.broadcastSyncRemover = null
      }
    }
  }
}

export default StoreSyncService.getInstance()
