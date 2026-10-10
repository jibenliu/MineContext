import { IpcChannel } from '@shared/ipc-channel'
import { act, render } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { useTrayRecordingSync } from './use-tray-recording-sync'

function Host() {
  useTrayRecordingSync()
  return null
}

describe('托盘状态同步', () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
    delete (window as any).electron
    delete (window as any).screenMonitorAPI
    delete (window as any).serverPushAPI
    delete (window as any).mcRuntime
  })

  it('挂载时拉 status，并在 SSE 推送时更新托盘（带 i18n 文案）', async () => {
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
    window.mcRuntime = {
      get: vi.fn().mockResolvedValue({ port: 19790, token: 't' })
    } as unknown as Window['mcRuntime']
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: true,
        json: async () => ({
          data: { components: { embedding: { status: 'ok' } } }
        })
      })
    )

    render(<Host />)

    await act(async () => {
      await Promise.resolve()
      await Promise.resolve()
    })

    expect(invoke).toHaveBeenCalledWith(
      IpcChannel.Tray_UpdateRecordingStatus,
      expect.objectContaining({
        recording: false,
        tooltip: expect.stringContaining('MineContext'),
        toggleLabel: expect.any(String)
      })
    )

    await act(async () => {
      statusHandler?.('running')
    })
    expect(invoke).toHaveBeenCalledWith(
      IpcChannel.Tray_UpdateRecordingStatus,
      expect.objectContaining({ recording: true })
    )
  })

  it('health 报 embedding.paused 时托盘提示带索引暂停', async () => {
    const invoke = vi.fn().mockResolvedValue(undefined)
    window.electron = { ipcRenderer: { invoke } } as unknown as Window['electron']
    window.screenMonitorAPI = {
      checkCanRecord: vi.fn().mockResolvedValue({ canRecord: true, status: 'running' })
    } as unknown as Window['screenMonitorAPI']
    window.serverPushAPI = {
      pushScreenMonitorStatus: () => () => undefined
    } as unknown as Window['serverPushAPI']
    window.mcRuntime = {
      get: vi.fn().mockResolvedValue({ port: 19790, token: 't' })
    } as unknown as Window['mcRuntime']
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: true,
        json: async () => ({
          data: { components: { embedding: { status: 'paused' } } }
        })
      })
    )

    render(<Host />)

    await act(async () => {
      await Promise.resolve()
      await Promise.resolve()
      await Promise.resolve()
    })

    const trayCalls = invoke.mock.calls.filter(([channel]) => channel === IpcChannel.Tray_UpdateRecordingStatus)
    const last = trayCalls.at(-1)?.[1] as { tooltip?: string }
    expect(last?.tooltip).toMatch(/索引已暂停|Indexing paused/)
  })

  it('backend:get-status 失败时标记断连文案', async () => {
    const invoke = vi.fn().mockImplementation(async (channel: string) => {
      if (channel === IpcChannel.Backend_GetStatus) {
        throw new Error('down')
      }
      return undefined
    })
    window.electron = { ipcRenderer: { invoke } } as unknown as Window['electron']
    window.screenMonitorAPI = {
      checkCanRecord: vi.fn().mockResolvedValue({ canRecord: true, status: 'stopped' })
    } as unknown as Window['screenMonitorAPI']
    window.serverPushAPI = {
      pushScreenMonitorStatus: () => () => undefined
    } as unknown as Window['serverPushAPI']
    window.mcRuntime = {
      get: vi.fn().mockRejectedValue(new Error('no runtime'))
    } as unknown as Window['mcRuntime']

    render(<Host />)

    await act(async () => {
      await Promise.resolve()
      await Promise.resolve()
      await Promise.resolve()
    })

    const trayCalls = invoke.mock.calls.filter(([channel]) => channel === IpcChannel.Tray_UpdateRecordingStatus)
    const last = trayCalls.at(-1)?.[1] as { tooltip?: string; title?: string }
    expect(last?.tooltip).toMatch(/无法连接本地服务|Local service unavailable/)
    expect(last?.title).toMatch(/断|!/)
  })
})
