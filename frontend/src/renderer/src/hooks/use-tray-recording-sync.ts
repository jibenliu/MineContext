// 托盘录制指示灯：挂在 app shell，不依赖屏幕监控页是否挂载。
// 页面卸载后若不再同步，托盘会一直显示旧状态。

import { IpcChannel } from '@shared/ipc-channel'
import { getLogger } from '@shared/logger/renderer'
import { useEffect } from 'react'

const logger = getLogger('tray-recording-sync')

function updateTray(recording: boolean): void {
  window.electron?.ipcRenderer?.invoke(IpcChannel.Tray_UpdateRecordingStatus, recording).catch((error: unknown) => {
    logger.error('Failed to update tray recording status:', error)
  })
}

/** 订阅采集 SSE + 启动时拉一次 status，把「正在录制」同步到托盘。 */
export function useTrayRecordingSync(): void {
  useEffect(() => {
    let monitoring = false
    let canRecord = true

    const sync = () => updateTray(monitoring && canRecord)

    const unsubscribe = window.serverPushAPI?.pushScreenMonitorStatus((status: unknown) => {
      monitoring = status === 'running'
      sync()
    })

    void window.screenMonitorAPI
      ?.checkCanRecord?.()
      .then((result: { canRecord?: boolean; status?: string } | undefined) => {
        if (result && typeof result.canRecord === 'boolean') {
          canRecord = result.canRecord
        }
        if (result?.status === 'running' || result?.status === 'stopped') {
          monitoring = result.status === 'running'
        }
        sync()
      })
      .catch((error: unknown) => {
        logger.error('Failed to read capture status for tray sync:', error)
      })

    return () => {
      unsubscribe?.()
    }
  }, [])
}
