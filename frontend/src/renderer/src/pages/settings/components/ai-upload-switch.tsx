// 设置页：允许 AI 出网（privacy.ai_upload）。默认关闭；打开后视觉/总结/对话才可能请求模型。

import { Message, Switch, Typography } from '@arco-design/web-react'
import { useI18n } from '@renderer/i18n'
import { getPrivacySettings, updatePrivacySettings } from '@renderer/services/privacy'
import { useCallback, useEffect, useState } from 'react'

const { Text } = Typography

export function AiUploadSwitch() {
  const { t } = useI18n()
  const [enabled, setEnabled] = useState(false)
  const [pending, setPending] = useState(true)

  useEffect(() => {
    let alive = true
    void (async () => {
      try {
        const privacy = await getPrivacySettings()
        if (alive) setEnabled(privacy.ai_upload)
      } catch {
        // 读失败时保持关，避免误显示为已开启
      } finally {
        if (alive) setPending(false)
      }
    })()
    return () => {
      alive = false
    }
  }, [])

  const onChange = useCallback(
    async (next: boolean) => {
      setPending(true)
      try {
        const privacy = await updatePrivacySettings({ ai_upload: next })
        setEnabled(privacy.ai_upload)
        Message.success(next ? t('settings.aiUpload.enabled') : t('settings.aiUpload.disabled'))
      } catch {
        Message.error(t('settings.aiUpload.failed'))
      } finally {
        setPending(false)
      }
    },
    [t]
  )

  return (
    <div
      id="ai-upload"
      className="flex max-w-[520px] items-center justify-between gap-6 py-1 scroll-mt-6"
      data-testid="ai-upload-switch">
      <div className="flex flex-col">
        <span className="text-[14px]">{t('settings.aiUpload')}</span>
        <Text type="secondary" className="!text-[12px]">
          {t('settings.aiUpload.hint')}
        </Text>
      </div>
      <Switch checked={enabled} disabled={pending} onChange={onChange} aria-label={t('settings.aiUpload')} />
    </div>
  )
}
