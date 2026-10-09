import axiosInstance from '@renderer/services/axios-config'
import { beforeEach, expect, it, vi } from 'vitest'

import { getStoredApiKey, isPlainApiKeyCandidate } from './settings'

vi.mock('@renderer/services/axios-config', () => ({
  default: { get: vi.fn() }
}))

it('空串与脱敏回显不算可复制明文', () => {
  expect(isPlainApiKeyCandidate('', 'sk-a••••••••wxyz')).toBe(false)
  expect(isPlainApiKeyCandidate('sk-a••••••••wxyz', 'sk-a••••••••wxyz')).toBe(false)
  expect(isPlainApiKeyCandidate('sk-ab••••••••yz', 'other')).toBe(false)
})

it('刚输入的真实密钥可直接复制', () => {
  expect(isPlainApiKeyCandidate('sk-live-secret-key', 'sk-l••••••••cret')).toBe(true)
  expect(isPlainApiKeyCandidate('sk-live-secret-key', '')).toBe(true)
})

beforeEach(() => {
  vi.mocked(axiosInstance.get).mockReset()
})

it('复制向量密钥时带 field=embedding 查询参数', async () => {
  vi.mocked(axiosInstance.get).mockResolvedValue({
    data: { code: 0, data: { apiKey: 'sk-embed-only' } }
  })

  await expect(getStoredApiKey('embedding')).resolves.toBe('sk-embed-only')
  expect(axiosInstance.get).toHaveBeenCalledWith('/api/model_settings/api_key', {
    params: { field: 'embedding' }
  })
})

it('复制视觉密钥时不带 field 参数（兼容旧契约）', async () => {
  vi.mocked(axiosInstance.get).mockResolvedValue({
    data: { code: 0, data: { apiKey: 'sk-vision-only' } }
  })

  await expect(getStoredApiKey('vision')).resolves.toBe('sk-vision-only')
  expect(axiosInstance.get).toHaveBeenCalledWith('/api/model_settings/api_key', {
    params: undefined
  })
})
