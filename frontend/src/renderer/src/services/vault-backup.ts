// 笔记树 + uploads 备份：走专用 HTTP（zip 二进制），不经 JSON 适配层。

import axiosInstance from './axios-config'

export interface VaultBackupImportResult {
  vault_count: number
  file_count: number
}

export interface VaultBackupApi {
  exportZip: () => Promise<Blob>
  importZipBase64: (data: string) => Promise<VaultBackupImportResult>
}

export function createVaultBackupApi(): VaultBackupApi {
  return {
    async exportZip() {
      const response = await axiosInstance.get<Blob>('/api/v1/vault/export', {
        responseType: 'blob'
      })
      return response.data
    },
    async importZipBase64(data: string) {
      const response = await axiosInstance.post<{
        code?: number
        data?: VaultBackupImportResult
        message?: string
      }>('/api/v1/vault/import', { data })
      const envelope = response.data
      if (typeof envelope?.code === 'number' && envelope.code !== 0) {
        throw new Error(envelope.message ?? 'import failed')
      }
      const result = envelope?.data
      if (typeof result?.vault_count !== 'number' || typeof result?.file_count !== 'number') {
        throw new Error('import response missing counts')
      }
      return result
    }
  }
}

export function blobToBase64(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => {
      const result = reader.result
      if (typeof result !== 'string') {
        reject(new Error('expected data URL'))
        return
      }
      const comma = result.indexOf(',')
      resolve(comma >= 0 ? result.slice(comma + 1) : result)
    }
    reader.onerror = () => reject(reader.error ?? new Error('read failed'))
    reader.readAsDataURL(blob)
  })
}

export function downloadBlob(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob)
  const link = document.createElement('a')
  link.href = url
  link.download = filename
  link.click()
  URL.revokeObjectURL(url)
}
