// 搜索页壳。
//
// 组件本身在 `components/search`，这里只做「页面级」该做的事：标题与布局。
// 单独一层的理由与其它页面一致：路由指向页面，页面再组装组件 ——
// 筛选器与结果分组这类页面级编排留在这里，组件保持可复用。

import { Typography } from '@arco-design/web-react'
import { SearchResults } from '@renderer/components/search/search-results'
import { useI18n } from '@renderer/i18n'
import { FC } from 'react'

const { Title } = Typography

export const SearchPage: FC = () => {
  const { t } = useI18n()
  return (
    <div className="search-page h-full w-full overflow-y-auto p-6">
      <Title heading={4} style={{ marginBottom: 16 }}>
        {t('search.title')}
      </Title>
      <SearchResults />
    </div>
  )
}
