import axiosInstance from '@renderer/services/axios-config'
import { afterEach, describe, expect, it, vi } from 'vitest'

import GlobalEventService from './global-event-service'

describe('global event polling', () => {
  afterEach(() => {
    GlobalEventService.getInstance().stopPolling()
    vi.useRealTimers()
  })

  it('不支持事件接口时停止轮询而不是反复读取 null', async () => {
    vi.useFakeTimers()
    const get = vi.spyOn(axiosInstance, 'get').mockResolvedValue({
      status: 200,
      data: { code: 1, error_code: 'not_implemented', data: null }
    })
    const dispatch = vi.fn()
    GlobalEventService.getInstance().startPolling(dispatch)
    await vi.advanceTimersByTimeAsync(90_000)
    expect(get).toHaveBeenCalledTimes(1)
    expect(dispatch).not.toHaveBeenCalled()
  })

  it('业务失败或无效事件不进入 Redux，后续成功仍可读取', async () => {
    const get = vi.spyOn(axiosInstance, 'get')
    const dispatch = vi.fn()
    const service = GlobalEventService.getInstance()
    for (const data of [null, { events: {} }, { events: [null] }]) {
      get.mockResolvedValueOnce({ status: 200, data: { code: 1, data } })
      await service.fetchEvents(dispatch)
    }
    expect(dispatch).not.toHaveBeenCalled()
    get.mockResolvedValueOnce({ status: 200, data: { code: 0, data: { events: [] } } })
    await service.fetchEvents(dispatch)
    expect(dispatch).toHaveBeenCalledTimes(1)
  })

  it('慢请求不叠加，成功信封中的无效事件被过滤', async () => {
    let finish!: (response: unknown) => void
    const get = vi.spyOn(axiosInstance, 'get').mockReturnValue(
      new Promise((resolve) => {
        finish = resolve
      })
    )
    const service = GlobalEventService.getInstance()
    const dispatch = vi.fn()
    const pending = service.fetchEvents(dispatch)
    await service.fetchEvents(dispatch)
    expect(get).toHaveBeenCalledTimes(1)
    finish({ status: 200, data: { code: 0, data: { events: [null, { id: 'broken' }] } } })
    await pending
    expect(dispatch).toHaveBeenCalledWith(expect.objectContaining({ payload: [] }))
  })
})
