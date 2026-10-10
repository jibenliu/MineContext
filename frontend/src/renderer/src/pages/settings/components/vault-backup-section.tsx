// 设置页「数据备份」：导出笔记树 + uploads；可选导入同格式 zip。

import { Button, Message, Typography, Upload } from '@arco-design/web-react'
import { useI18n } from '@renderer/i18n'
import {
  blobToBase64,
  createVaultBackupApi,
  downloadBlob,
  type VaultBackupApi
} from '@renderer/services/vault-backup'
import { useCallback, useState } from 'react'

const { Text } = Typography

export function VaultBackupSection({ api }: { api?: VaultBackupApi }) {
  const { t } = useI18n()
  const [client] = useState(() => api ?? createVaultBackupApi())
  const [exporting, setExporting] = useState(false)
  const [importing, setImporting] = useState(false)
  const [status, setStatus] = useState('')

  const onExport = useCallback(async () => {
    setExporting(true)
    setStatus('')
    try {
      const blob = await client.exportZip()
      downloadBlob(blob, 'minecontext-vault-backup.zip')
      setStatus(t('settings.vaultBackup.exportDone'))
      Message.success(t('settings.vaultBackup.exportDone'))
    } catch {
      Message.error(t('settings.vaultBackup.exportFailed'))
    } finally {
      setExporting(false)
    }
  }, [client, t])

  const onImportFile = useCallback(
    async (file: File) => {
      setImporting(true)
      setStatus('')
      try {
        const data = await blobToBase64(file)
        const result = await client.importZipBase64(data)
        setStatus(
          t('settings.vaultBackup.importDone', {
            vaults: result.vault_count,
            files: result.file_count
          })
        )
        Message.success(t('settings.vaultBackup.importDone', {
          vaults: result.vault_count,
          files: result.file_count
        }))
      } catch {
        Message.error(t('settings.vaultBackup.importFailed'))
      } finally {
        setImporting(false)
      }
      return false
    },
    [client, t]
  )

  return (
    <div className="mt-[12px] flex max-w-[520px] flex-col gap-2 py-1" data-testid="vault-backup-section">
      <div className="flex flex-col">
        <span className="text-[14px] font-bold text-[var(--color-text-1)]">{t('settings.vaultBackup')}</span>
        <Text type="secondary" className="!text-[12px]">
          {t('settings.vaultBackup.hint')}
        </Text>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Button
          type="secondary"
          className="mc-secondary-btn"
          loading={exporting}
          data-testid="vault-backup-export"
          onClick={() => void onExport()}>
          {t('settings.vaultBackup.export')}
        </Button>
        <Upload
          accept=".zip,application/zip"
          showUploadList={false}
          disabled={importing}
          beforeUpload={(file) => {
            void onImportFile(file)
            return false
          }}>
          <Button
            type="secondary"
            className="mc-secondary-btn"
            loading={importing}
            data-testid="vault-backup-import">
            {t('settings.vaultBackup.import')}
          </Button>
        </Upload>
      </div>
      {status ? (
        <Text type="secondary" className="!text-[12px]" role="status">
          {status}
        </Text>
      ) : null}
    </div>
  )
}
