// axios 路径的凭据注入：端口与 token 必须**一起**写进去。
//
// 为什么值得一条测试：设置页 / 会话 / 消息走的是 axios 而不是适配层，
// 凭据是启动流程运行时注入的（`setHttpClientConfigurator`）。只注入端口不注入 token
// （或注入晚于首个请求）的表现是 401，而界面上只显示「保存失败」——
// 计划里把它列为"注入时序脆弱"，这里把注入点与启动流程的接线都钉住。

import { installHttpBackendFromRuntime } from '@renderer/adapters/install'
import { afterEach, describe, expect, it } from 'vitest'

import axiosInstance, { configureHttpClient } from './axios-config'

const originalFetch = globalThis.fetch
const originalBaseURL = axiosInstance.defaults.baseURL
const originalToken = axiosInstance.defaults.headers.common['X-MC-Token']

afterEach(() => {
  globalThis.fetch = originalFetch
  axiosInstance.defaults.baseURL = originalBaseURL
  axiosInstance.defaults.headers.common['X-MC-Token'] = originalToken
})

/** 只记录请求、不真发（SSE 工厂会用到 fetch，这里给个空流）。 */
function stubFetch(): void {
  globalThis.fetch = (async () => ({
    status: 200,
    ok: true,
    text: async () => '',
    body: null
  })) as unknown as typeof fetch
}

describe('axios 凭据注入（rust 后端）', () => {
  it('configureHttpClient 同时写入端口与 token', () => {
    configureHttpClient(4321, 'tok-abc')

    expect(axiosInstance.defaults.baseURL).toBe('http://127.0.0.1:4321')
    expect(axiosInstance.defaults.headers.common['X-MC-Token']).toBe('tok-abc')
  })

  it('启动流程装适配层时，axios 路径也拿到同一份凭据', () => {
    stubFetch()

    installHttpBackendFromRuntime({ port: 5333, token: 'tok-from-runtime' })

    expect(axiosInstance.defaults.baseURL).toBe('http://127.0.0.1:5333')
    expect(axiosInstance.defaults.headers.common['X-MC-Token']).toBe('tok-from-runtime')
  })
})
