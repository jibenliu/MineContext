// 设置页入口：跳转到记忆洞察（风险 / 客户 / 学习 / 交接）。

import { Button, Typography } from '@arco-design/web-react'
import { useNavigation } from '@renderer/hooks/use-navigation'
import { useI18n } from '@renderer/i18n'

const { Text } = Typography

export function InsightsEntrySection({ onOpen }: { onOpen?: () => void }) {
  const { t } = useI18n()
  const { navigateToMainTab } = useNavigation()
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
          navigateToMainTab('insights', '/insights')
        }}>
        {t('insights.title')}
      </Button>
    </div>
  )
}
