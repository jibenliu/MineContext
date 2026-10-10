// 设置页入口：跳转到记忆洞察（风险 / 客户 / 学习 / 交接）。
// 引导遮罩里的 Settings 不在 HashRouter 内，因此用 hash 跳转而不是 useNavigate。

import { Button, Typography } from '@arco-design/web-react'
import { useI18n } from '@renderer/i18n'

const { Text } = Typography

export function InsightsEntrySection({ onOpen }: { onOpen?: () => void }) {
  const { t } = useI18n()
  return (
    <div className="mb-4" data-testid="settings-insights-entry">
      <Text type="secondary" className="mb-2 block text-[13px]">
        {t('insights.subtitle')}
      </Text>
      <Button
        type="outline"
        size="small"
        data-testid="settings-open-insights"
        onClick={() => {
          onOpen?.()
          window.location.hash = '#/insights'
        }}>
        {t('insights.title')}
      </Button>
    </div>
  )
}
