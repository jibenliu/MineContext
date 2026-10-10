// 设置页「导出诊断包」：一键写出脱敏 zip，方便贴 issue / 发给支持。

import { Button, Message, Typography } from '@arco-design/web-react'
import { useI18n } from '@renderer/i18n'
import { useCallback, useMemo, useState } from 'react'

import { createExportDiagnostics, type ExportDiagnostics } from '../../../adapters/export-diagnostics'

const { Text } = Typography

export function DiagnosticExportSection({ api }: { api?: ExportDiagnostics }) {
  const { t } = useI18n()
  const client = useMemo(() => api ?? createExportDiagnostics(globalThis), [api])
  const [pending, setPending] = useState(false)
  const unsupported = client.shell === 'none'

  const onExport = useCallback(async () => {
    setPending(true)
    try {
      const result = await client.export()
      Message.success(t('settings.diagnosticExport.success', { path: result.path }))
    } catch {
      Message.error(t('settings.diagnosticExport.failed'))
    } finally {
      setPending(false)
    }
  }, [client, t])

  return (
    <div className="mt-[12px] flex max-w-[520px] flex-col gap-2 py-1" data-testid="diagnostic-export-section">
      <div className="flex flex-col">
        <span className="text-[14px] font-bold text-[var(--color-text-1)]">{t('settings.diagnosticExport')}</span>
        <Text type="secondary" className="!text-[12px]">
          {unsupported ? t('settings.diagnosticExport.unsupported') : t('settings.diagnosticExport.hint')}
        </Text>
      </div>
      <div>
        <Button
          type="secondary"
          className="mc-secondary-btn"
          data-testid="diagnostic-export-button"
          loading={pending}
          disabled={unsupported || pending}
          onClick={() => void onExport()}>
          {t('settings.diagnosticExport.run')}
        </Button>
      </div>
    </div>
  )
}
