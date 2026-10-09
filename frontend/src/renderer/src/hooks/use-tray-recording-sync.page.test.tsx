import { IpcChannel } from '@shared/ipc-channel'
import { act, render } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import { useTrayRecordingSync } from './use-tray-recording-sync'

function Host() {
  useTrayRecordingSync()
  return null
}

describe('托盘录制状态同步', () => {
  it('挂载时拉 status，并在 SSE 推送时更新托盘', async () => {
    const invoke = vi.fn().mockResolvedValue(undefined)
    let statusHandler: ((status: unknown) => void) | undefined
    window.electron = { ipcRenderer: { invoke } } as unknown as Window['electron']
    window.screenMonitorAPI = {
      checkCanRecord: vi.fn().mockResolvedValue({ canRecord: true, status: 'stopped' })
    } as unknown as Window['screenMonitorAPI']
    window.serverPushAPI = {
      pushScreenMonitorStatus: (callback: (status: unknown) => void) => {
        statusHandler = callback
        return () => {
          statusHandler = undefined
        }
      }
    } as unknown as Window['serverPushAPI']

    render(<Host />)

    await act(async () => {
      await Promise.resolve()
    })
    expect(invoke).toHaveBeenCalledWith(IpcChannel.Tray_UpdateRecordingStatus, false)

    await act(async () => {
      statusHandler?.('running')
    })
    expect(invoke).toHaveBeenCalledWith(IpcChannel.Tray_UpdateRecordingStatus, true)
  })
})
