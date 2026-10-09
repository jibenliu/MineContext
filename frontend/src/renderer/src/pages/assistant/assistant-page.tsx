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
    // 与设置页同款：侧栏右侧整块卡片铺满（flex-1 + min-w-0），避免浮在外壳底色上
    // 右侧留出大片黑/灰空带。外层不再叠 pr-2（路由主栏已有）。
    <div className="assistant-page flex h-full min-h-0 w-full min-w-0 flex-1 flex-col overflow-hidden pb-2 pl-0">
      <div style={{ height: 8 }} />
      <div className="assistant-page-surface flex min-h-0 w-full min-w-0 flex-1 flex-col self-stretch overflow-hidden rounded-[16px] bg-white px-4 py-5 md:px-6">
        <Title heading={4} style={{ marginBottom: 16 }}>
          {t('assistant.title')}
        </Title>
        <div className="flex min-h-0 w-full min-w-0 flex-1 self-stretch">
          <AssistantStream />
        </div>
      </div>
    </div>
  )
}
