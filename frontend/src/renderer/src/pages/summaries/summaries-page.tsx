// 总结页壳：任意时段入口 + 最近的总结。
//
// 组件本身在 `components/`，这里只做页面级的事（标题与布局）——
// 模板选择、导出这类编排属于页面，组件保持可复用。

import { Typography } from '@arco-design/web-react'
import { AdhocSummary } from '@renderer/components/adhoc-summary/adhoc-summary'
import { ErrorBoundary } from '@renderer/components/error-boundary'
import { SummaryCard } from '@renderer/components/summary-card/summary-card'
import { useI18n } from '@renderer/i18n'
import { FC } from 'react'

const { Title } = Typography

export const SummariesPage: FC = () => {
  const { t } = useI18n()
  return (
    <div className="summaries-page h-full w-full overflow-y-auto p-6">
      <Title heading={4} style={{ marginBottom: 16 }}>
        {t('summary.title')}
      </Title>
      <AdhocSummary />

      <Title heading={4} style={{ marginTop: 28, marginBottom: 16 }}>
        {t('summary.recent')}
      </Title>
      <ErrorBoundary title={t('summary.loadFailed')}>
        <SummaryCard />
      </ErrorBoundary>
    </div>
  )
}
