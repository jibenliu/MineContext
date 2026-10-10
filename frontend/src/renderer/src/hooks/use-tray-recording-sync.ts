// 托盘状态指示：挂在 app shell，不依赖屏幕监控页是否挂载。
// 扩展既有录制同步：加上索引暂停（health embedding.paused）与 daemon 断连。

import { fetchInitCheckFromHealth } from '@renderer/adapters/init-check-health'
import { getLocale } from '@renderer/i18n'
import { IpcChannel } from '@shared/ipc-channel'
import { getLogger } from '@shared/logger/renderer'
import { useEffect } from 'react'

import { indexingPausedFromHealth, presentTrayStatus, type TrayStatusFlags } from './tray-status'

const logger = getLogger('tray-recording-sync')

/** 与 channel-map 对 backend:status-changed 的约定一致：轻量轮询即可。 */
const POLL_MS = 3000

function pushTray(flags: TrayStatusFlags): void {
  const presentation = presentTrayStatus(flags, getLocale())
  window.electron?.ipcRenderer
    ?.invoke(IpcChannel.Tray_UpdateRecordingStatus, {
      recording: presentation.recording,
      tooltip: presentation.tooltip,
      toggleLabel: presentation.toggleLabel,
      title: presentation.title
    })
    .catch((error: unknown) => {
      logger.error('Failed to update tray status:', error)
    })
}

/** 订阅采集 SSE，并轮询 daemon / health，把托盘提示、菜单与短标题同步出去。 */
export function useTrayRecordingSync(): void {
  useEffect(() => {
    const flags: TrayStatusFlags = {
      recording: false,
      indexingPaused: false,
      daemonDisconnected: false
    }

    const sync = () => pushTray(flags)

    const unsubscribe = window.serverPushAPI?.pushScreenMonitorStatus((status: unknown) => {
      flags.recording = status === 'running'
      sync()
    })

    void window.screenMonitorAPI
      ?.checkCanRecord?.()
      .then((result: { canRecord?: boolean; status?: string } | undefined) => {
        if (result?.status === 'running' || result?.status === 'stopped') {
          flags.recording = result.status === 'running' && result.canRecord !== false
        }
        sync()
      })
      .catch((error: unknown) => {
        logger.error('Failed to read capture status for tray sync:', error)
      })

    let cancelled = false
    const poll = async () => {
      try {
        await window.electron?.ipcRenderer?.invoke(IpcChannel.Backend_GetStatus)
        if (cancelled) return
        flags.daemonDisconnected = false
      } catch (error: unknown) {
        if (cancelled) return
        flags.daemonDisconnected = true
        logger.warn('Tray poll: daemon unreachable', error)
      }

      try {
        const health = await fetchInitCheckFromHealth({
          getRuntime: async () => {
            try {
              return (await window.mcRuntime?.get?.()) ?? null
            } catch {
              return null
            }
          },
          fetch: globalThis.fetch.bind(globalThis)
        })
        if (cancelled) return
        flags.indexingPaused = indexingPausedFromHealth(health)
        // 能拉到 health 说明控制面可达
        flags.daemonDisconnected = false
      } catch {
        if (cancelled) return
        // health 失败且 status 也失败时保持断连；仅 health 失败不覆盖 status 成功
      }

      sync()
    }

    void poll()
    const timer = window.setInterval(() => {
      void poll()
    }, POLL_MS)

    return () => {
      cancelled = true
      window.clearInterval(timer)
      unsubscribe?.()
    }
  }, [])
}
