// 助手页壳。
//
// 与搜索页同样的分层：路由指向页面，页面组装组件。
// 这里只挂流式气泡；完整会话界面（markdown、历史消息）由这一层组装，
// 气泡本身不需要知道页面结构。

import { Typography } from '@arco-design/web-react'
import { AssistantStream } from '@renderer/components/assistant-stream/assistant-stream'
import { useI18n } from '@renderer/i18n'
import { FC } from 'react'

const { Title } = Typography

export const AssistantPage: FC = () => {
  const { t } = useI18n()
  return (
    // 高度用 flex 分配：页面自己 `overflow-y-auto` 时，`h-full` 的子节点加上
    // 外边距与标题高度必然超出页面高度，于是**空态也会顶出一条滚动条**。
    // 滚动只留给消息区（`AssistantStream` 内部自己维护）。
    <div className="assistant-page flex h-full w-full flex-col p-6">
      <Title heading={4} style={{ marginBottom: 16 }}>
        {t('assistant.title')}
      </Title>
      <div className="flex min-h-0 flex-1">
        <AssistantStream />
      </div>
    </div>
  )
}
