// 设置页「截图占用」：展示近似磁盘用量，并保存保留天数 / 容量上限。
// 只作用于采集截图 blob，不会动 vault 笔记或 uploads。

import { Button, InputNumber, Message, Typography } from '@arco-design/web-react'
import { useI18n } from '@renderer/i18n'
import { useCallback, useEffect, useState } from 'react'

const { Text } = Typography

export type CaptureRetentionConfig = {
  retentionDays: number
  maxTotalGb: number
  diskUsage: { totalBytes: number; blobCount: number } | null
}

export interface RetentionApi {
  getConfig: () => Promise<CaptureRetentionConfig | undefined>
  saveConfig: (patch: { retentionDays: number; maxTotalGb: number }) => Promise<unknown>
}

function createRetentionApi(): RetentionApi {
  const api = (globalThis as { retentionApi?: RetentionApi }).retentionApi
  if (!api) {
    return {
      getConfig: async () => undefined,
      saveConfig: async () => undefined
    }
  }
  return api
}

export function formatApproxBytes(bytes: number, _locale = 'en'): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '—'
  const units = ['B', 'KB', 'MB', 'GB', 'TB'] as const
  let value = bytes
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
  }
  if (unit === 0) return `${Math.round(value)} ${units[unit]}`
  const text = value >= 10 ? value.toFixed(1) : value.toFixed(2)
  return `${text.replace(/\.?0+$/, '')} ${units[unit]}`
}

function normalizeConfig(raw: unknown): CaptureRetentionConfig | undefined {
  if (!raw || typeof raw !== 'object') return undefined
  const row = raw as Record<string, unknown>
  const retentionDays = Number(row.retentionDays ?? row.retention_days ?? 7)
  const maxTotalGb = Number(row.maxTotalGb ?? row.max_total_gb ?? 10)
  const usage = row.disk_usage ?? row.diskUsage
  let diskUsage: CaptureRetentionConfig['diskUsage'] = null
  if (usage && typeof usage === 'object') {
    const u = usage as Record<string, unknown>
    const totalBytes = Number(u.total_bytes ?? u.totalBytes ?? 0)
    const blobCount = Number(u.blob_count ?? u.blobCount ?? 0)
    if (Number.isFinite(totalBytes) && Number.isFinite(blobCount)) {
      diskUsage = { totalBytes, blobCount }
    }
  }
  if (!Number.isFinite(retentionDays) || !Number.isFinite(maxTotalGb)) return undefined
  return { retentionDays, maxTotalGb, diskUsage }
}

export function RetentionSection({ api }: { api?: RetentionApi }) {
  const { t, locale } = useI18n()
  const [client] = useState(() => api ?? createRetentionApi())
  const [retentionDays, setRetentionDays] = useState(7)
  const [maxTotalGb, setMaxTotalGb] = useState(10)
  const [diskUsage, setDiskUsage] = useState<CaptureRetentionConfig['diskUsage']>(null)
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)

  const refresh = useCallback(async () => {
    setLoading(true)
    try {
      const config = normalizeConfig(await client.getConfig())
      if (!config) {
        setDiskUsage(null)
        return
      }
      setRetentionDays(config.retentionDays)
      setMaxTotalGb(config.maxTotalGb)
      setDiskUsage(config.diskUsage)
    } finally {
      setLoading(false)
    }
  }, [client])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const save = useCallback(async () => {
    if (!Number.isFinite(retentionDays) || retentionDays < 0) {
      Message.error(t('settings.retention.invalid'))
      return
    }
    if (!Number.isFinite(maxTotalGb) || maxTotalGb < 0) {
      Message.error(t('settings.retention.invalid'))
      return
    }
    setSaving(true)
    try {
      await client.saveConfig({
        retentionDays: Math.floor(retentionDays),
        maxTotalGb
      })
      Message.success(t('settings.retention.saved'))
      await refresh()
    } catch {
      Message.error(t('settings.retention.failed'))
    } finally {
      setSaving(false)
    }
  }, [client, maxTotalGb, refresh, retentionDays, t])

  const usageLabel =
    diskUsage == null
      ? t('settings.retention.usageUnknown')
      : t('settings.retention.usage', {
          size: formatApproxBytes(diskUsage.totalBytes, locale),
          count: String(diskUsage.blobCount)
        })

  return (
    <div className="mt-[16px]" data-testid="retention-section">
      <div className="mb-[4px] text-[13px] font-medium text-[var(--color-text-1)]">{t('settings.retention')}</div>
      <Text type="secondary" className="mb-[8px] block text-[12px]">
        {t('settings.retention.hint')}
      </Text>
      <Text className="mb-[10px] block text-[13px] text-[var(--color-text-2)]" data-testid="retention-disk-usage">
        {loading ? t('common.loading') : usageLabel}
      </Text>
      <div className="flex flex-wrap items-end gap-3">
        <label className="flex flex-col gap-1 text-[12px] text-[var(--color-text-3)]">
          {t('settings.retention.days')}
          <InputNumber
            data-testid="retention-days"
            min={0}
            precision={0}
            value={retentionDays}
            onChange={(value) => setRetentionDays(Number(value ?? 0))}
            style={{ width: 120 }}
          />
        </label>
        <label className="flex flex-col gap-1 text-[12px] text-[var(--color-text-3)]">
          {t('settings.retention.maxGb')}
          <InputNumber
            data-testid="retention-max-gb"
            min={0}
            step={0.5}
            precision={2}
            value={maxTotalGb}
            onChange={(value) => setMaxTotalGb(Number(value ?? 0))}
            style={{ width: 120 }}
          />
        </label>
        <Button type="secondary" loading={saving} disabled={saving || loading} onClick={() => void save()}>
          {t('settings.retention.save')}
        </Button>
      </div>
    </div>
  )
}
