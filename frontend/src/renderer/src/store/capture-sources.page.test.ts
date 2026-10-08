import { configureStore } from '@reduxjs/toolkit'
import { installFakeBackend } from '@renderer/test/page-setup'
import { describe, expect, it, vi } from 'vitest'

import captureSources, { refreshCaptureSources, refreshCaptureSourcesFromSettings } from './capture-sources'

const createStore = () => configureStore({ reducer: { captureSources } })
const display = { id: 'screen:1', name: 'Display', type: 'screen', isVisible: true }

describe('采集源 Redux 状态', () => {
  it('创建 store 不取数，显式刷新后分类并合并应用窗口', async () => {
    const backend = installFakeBackend({
      'screen-monitor:get-capture-all-sources': {
        success: true,
        sources: [display, { id: 'window:1', name: 'Page - Chrome', type: 'window' }]
      }
    })
    const store = createStore()
    expect(backend.calls).toHaveLength(0)
    expect(store.getState().captureSources.available.state).toBe('idle')
    await store.dispatch(refreshCaptureSources()).unwrap()
    expect(store.getState().captureSources.available.data.screenSources).toEqual([display])
    expect(store.getState().captureSources.available.data.appSources[0].appName).toBe('Google Chrome')
  })

  it('保存的来源与当前可选来源独立刷新，兼容空设置', async () => {
    installFakeBackend({ 'screen-monitor:get-settings': { screenList: [display] } })
    const store = createStore()
    await store.dispatch(refreshCaptureSourcesFromSettings()).unwrap()
    expect(store.getState().captureSources.saved.data).toEqual({ screenSources: [display], appSources: [] })
    expect(store.getState().captureSources.available.state).toBe('idle')
    installFakeBackend({ 'screen-monitor:get-settings': null })
    await store.dispatch(refreshCaptureSourcesFromSettings()).unwrap()
    expect(store.getState().captureSources.saved.data.screenSources).toEqual([])
  })

  it('失败进入可重试状态，成功重试清除错误', async () => {
    installFakeBackend({ 'screen-monitor:get-capture-all-sources': { success: false, error: 'denied' } })
    const store = createStore()
    await store.dispatch(refreshCaptureSources())
    expect(store.getState().captureSources.available).toMatchObject({ state: 'hasError', error: 'denied' })
    installFakeBackend({ 'screen-monitor:get-capture-all-sources': { success: true, sources: [display] } })
    await store.dispatch(refreshCaptureSources()).unwrap()
    expect(store.getState().captureSources.available).toMatchObject({ state: 'hasData', error: null })
  })

  it('旧刷新晚返回不覆盖新结果或错误状态', async () => {
    installFakeBackend()
    let resolveOld!: (value: Awaited<ReturnType<typeof window.screenMonitorAPI.getCaptureAllSources>>) => void
    vi.spyOn(window.screenMonitorAPI, 'getCaptureAllSources')
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveOld = resolve
          })
      )
      .mockResolvedValueOnce({ success: true, sources: [display] })
    const store = createStore()
    const oldRequest = store.dispatch(refreshCaptureSources())
    await store.dispatch(refreshCaptureSources()).unwrap()
    resolveOld({ success: false, error: 'stale error' })
    await oldRequest
    expect(store.getState().captureSources.available).toMatchObject({
      state: 'hasData',
      error: null,
      data: { screenSources: [display] }
    })
  })

  it('取消刷新后忽略迟到响应，下次刷新仍能成功', async () => {
    installFakeBackend()
    let resolveOld!: (value: Awaited<ReturnType<typeof window.screenMonitorAPI.getCaptureAllSources>>) => void
    vi.spyOn(window.screenMonitorAPI, 'getCaptureAllSources')
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveOld = resolve
          })
      )
      .mockResolvedValueOnce({ success: true, sources: [display] })
    const store = createStore()
    const request = store.dispatch(refreshCaptureSources())
    request.abort()
    await request
    await store.dispatch(refreshCaptureSources()).unwrap()
    resolveOld({ success: true, sources: [] })
    await Promise.resolve()
    expect(store.getState().captureSources.available.data.screenSources).toEqual([display])
  })

  it('旧设置响应不覆盖最近选择，设置读取失败不改写已保存数据', async () => {
    installFakeBackend()
    let resolveOld!: (value: unknown) => void
    const getSettings = vi
      .spyOn(window.screenMonitorAPI, 'getSettings')
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveOld = resolve
          })
      )
      .mockResolvedValueOnce({ screenList: [display] })
    const store = createStore()
    const oldRequest = store.dispatch(refreshCaptureSourcesFromSettings())
    await store.dispatch(refreshCaptureSourcesFromSettings()).unwrap()
    resolveOld({ screenList: [] })
    await oldRequest
    expect(store.getState().captureSources.saved.data.screenSources).toEqual([display])
    getSettings.mockRejectedValueOnce(new Error('settings unavailable'))
    await store.dispatch(refreshCaptureSourcesFromSettings())
    expect(store.getState().captureSources.saved).toMatchObject({
      state: 'hasError',
      data: { screenSources: [display] }
    })
  })
})
