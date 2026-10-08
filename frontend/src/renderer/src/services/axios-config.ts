// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import { setHttpClientConfigurator } from '@renderer/adapters/install'
import { getLogger } from '@shared/logger/renderer'
import axios from 'axios'

const logger = getLogger('axiosConfig')

// baseURL 由启动流程按 `runtime.json` 里的端口写入（见 adapters/install 的
// setHttpClientConfigurator）。这里**不留固定端口兜底**：写死一个端口会让
// 「还没读到 runtime.json」表现成打到别的进程上，而错误信息只会说连不上/401 ——
// 排查方向于是从「地址还没注入」偏到「daemon 有问题」。
const axiosInstance = axios.create({
  baseURL: undefined,
  timeout: 60000,
  headers: {
    'Content-Type': 'application/json'
  },
  // Configuration to solve CORS issues
  withCredentials: false
})

// Function to dynamically update the baseURL
export const updateBaseURL = (port: number) => {
  const newBaseURL = `http://127.0.0.1:${port}`
  axiosInstance.defaults.baseURL = newBaseURL
}

/**
 * 由适配层把这两个值交过来：端口来自 daemon 的
 * `runtime.json`，token 是每次启动生成的 loopback 凭据。
 *
 * 没有它，设置页会打到未就绪的地址上**且不带 token**（daemon 直接 401）。
 */
export const configureHttpClient = (port: number, token: string): void => {
  axiosInstance.defaults.baseURL = `http://127.0.0.1:${port}`
  axiosInstance.defaults.headers.common['X-MC-Token'] = token
}

// 反向注册：适配层不 import 业务模块，业务模块把自己的配置函数交给它。
setHttpClientConfigurator(configureHttpClient)

// 适配层装好时已经通过 `configureHttpClient` 指过一次；这里再兜一次底：
// 端口**和 token** 都从 daemon 写的 `runtime.json` 取 —— 只更新 baseURL 的话，
// 适配层安装被跳过/失败时这条路径会一直 401，而用户在界面上只看到「保存失败」。
if (typeof window !== 'undefined' && window.mcRuntime?.get) {
  window.mcRuntime
    .get()
    .then((runtime) => {
      if (!runtime) return
      if (runtime.port && runtime.token) {
        configureHttpClient(runtime.port, runtime.token)
      } else if (runtime.port) {
        updateBaseURL(runtime.port)
      }
    })
    .catch((error) => {
      logger.warn('尚未拿到 daemon 端口与凭据，请求会在它就绪后才有地址', error)
    })
}

// Request interceptor
axiosInstance.interceptors.request.use(
  (config) => {
    if (!axiosInstance.defaults.baseURL) {
      return Promise.reject(new Error('后端地址尚未就绪（还没读到 daemon 的 runtime.json）'))
    }
    return config
  },
  (error) => {
    // Request error
    return Promise.reject(error)
  }
)

// Response interceptor
axiosInstance.interceptors.response.use(
  (response) => {
    // Response data
    return response
  },
  (error) => {
    // 401 时把原因说清楚：这条路径的凭据是运行时注入的，注入失败时的症状是
    // "设置保存不了"，而错误本身只会说 401 —— 排查时会往权限而不是注入上找。
    if (error?.response?.status === 401) {
      logger.warn('[mc] 后端拒绝了这次请求（401）：运行时凭据可能还没注入（daemon 未就绪或适配层未安装）。')
    }
    return Promise.reject(error)
  }
)

export default axiosInstance
