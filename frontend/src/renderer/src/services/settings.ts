// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import axiosInstance from '@renderer/services/axios-config'
import { get } from 'lodash'

// Model configuration interface
export interface ModelConfigProps {
  modelPlatform: string // Model platform, e.g., doubao, openai, custom
  modelId: string // VLM model ID
  baseUrl: string // API base URL
  embeddingModelId: string // Embedding model ID
  apiKey: string // API key
  embeddingBaseUrl?: string // Optional separate embedding base URL
  embeddingApiKey?: string // Optional separate embedding API key
  embeddingModelPlatform?: string // Optional separate embedding platform
}

/** 单个平台已存凭据的脱敏视图（明文不进 get） */
export interface ProviderSettingsMask {
  modelId?: string
  baseUrl?: string
  embeddingModelId?: string
  embeddingBaseUrl?: string
  hasApiKey?: boolean
  apiKeyMasked?: string
  hasEmbeddingApiKey?: boolean
  embeddingApiKeyMasked?: string
  sharedEmbeddingWithVision?: boolean
}

// API response data structure
export interface ModelInfoResponseData {
  config: ModelConfigProps
  /** 是否已保存过密钥（get 的 apiKey 字段始终为空） */
  hasApiKey?: boolean
  /** 脱敏回显，例如 sk-l••••••••6789 */
  apiKeyMasked?: string
  /** 自建独立向量密钥是否已保存（没有则回退到视觉密钥） */
  hasEmbeddingApiKey?: boolean
  /** 向量密钥脱敏回显；与视觉相同时可与 apiKeyMasked 相同 */
  embeddingApiKeyMasked?: string
  /**
   * 各平台分档脱敏视图。切换并保存 B 不会抹掉 A；
   * 设置页用它回填非活跃平台的密钥框。
   */
  providers?: Record<string, ProviderSettingsMask>
}

// Complete API response structure
export interface ApiResponse<T> {
  code: number
  status: number
  message: string
  data: T
}

// Get model settings information
export const getModelInfo = async (): Promise<ModelInfoResponseData | undefined> => {
  const res = await axiosInstance.get<ModelInfoResponseData>('/api/model_settings/get')
  return get(res, 'data.data')
}

export type StoredApiKeyField = 'vision' | 'embedding'

/** 设置页「复制」：显式取已存明文（本机 + token）。找不到时返回空串，其它错误原样抛出。 */
export const getStoredApiKey = async (field: StoredApiKeyField = 'vision', provider?: string): Promise<string> => {
  try {
    const params: Record<string, string> = {}
    if (field === 'embedding') {
      params.field = 'embedding'
    }
    if (provider && provider.trim()) {
      params.provider = provider.trim()
    }
    const res = await axiosInstance.get<{ apiKey?: string }>('/api/model_settings/api_key', {
      params: Object.keys(params).length ? params : undefined
    })
    const code = get(res, 'data.code')
    const key = String(get(res, 'data.data.apiKey') || '').trim()
    // 兼容面偶发 HTTP 200 + code≠0
    if (code !== undefined && code !== 0) {
      return ''
    }
    return key
  } catch (error: unknown) {
    const status = get(error, 'response.status')
    if (status === 404) {
      return ''
    }
    throw error
  }
}

/** 输入框里是否已是可复制的明文（不是脱敏回显）。 */
export const isPlainApiKeyCandidate = (value: string, masked: string): boolean => {
  const text = value.trim()
  if (!text) return false
  if (masked && text === masked) return false
  // 脱敏串中间是 •；误把脱敏串当明文复制没有意义
  if (text.includes('•')) return false
  return true
}

// 模型设置写入接口的响应形状
export interface UpdateModelSettingsResponseData {
  success: boolean
  message: string
}

export const updateModelSettingsAPI = async (
  params: ModelConfigProps
): Promise<UpdateModelSettingsResponseData | undefined> => {
  const res = await axiosInstance.post<UpdateModelSettingsResponseData>('/api/model_settings/update', {
    config: {
      ...params
    }
  })
  return get(res, 'data.data')
}
