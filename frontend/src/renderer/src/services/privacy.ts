import axiosInstance from '@renderer/services/axios-config'
import { get } from 'lodash'

export interface PrivacySettings {
  ai_upload: boolean
  ai_enabled: boolean
}

export async function getPrivacySettings(): Promise<PrivacySettings> {
  const res = await axiosInstance.get<PrivacySettings>('/api/privacy')
  const data = get(res, 'data.data') as PrivacySettings | undefined
  return {
    ai_upload: Boolean(data?.ai_upload),
    ai_enabled: data?.ai_enabled !== false
  }
}

export async function updatePrivacySettings(
  patch: Partial<Pick<PrivacySettings, 'ai_upload' | 'ai_enabled'>>
): Promise<PrivacySettings> {
  const res = await axiosInstance.put<PrivacySettings>('/api/privacy', patch)
  const data = get(res, 'data.data') as PrivacySettings | undefined
  return {
    ai_upload: Boolean(data?.ai_upload),
    ai_enabled: data?.ai_enabled !== false
  }
}
