import { Typography } from '@arco-design/web-react'
import chatHistoryIcon from '@renderer/assets/icons/ai-assistant/chat-history.svg'
import { useNavigation } from '@renderer/hooks/use-navigation'
import { useI18n } from '@renderer/i18n'
import { ConversationResponse, conversationService } from '@renderer/services/conversation-service'
import { useAppDispatch } from '@renderer/store'
import { setActiveConversationId, toggleCreationAiAssistant, toggleHomeAiAssistant } from '@renderer/store/chat-history'
import { formatRelativeTime } from '@renderer/utils/time'
import { useMemoizedFn, useMount, useRequest } from 'ahooks'
import { isEmpty } from 'lodash'

import { CardLayout } from '../layout'
const ChatCard = () => {
  const { t } = useI18n()
  const { data: conversationList, run } = useRequest(
    async () => {
      const res = await conversationService.getConversationList({
        limit: 30,
        offset: 0,
        status: 'active'
      })
      return res.items || []
    },
    { manual: true }
  )
  useMount(() => {
    run()
  })
  const dispatch = useAppDispatch()
  const { navigateToMainTab, navigateToVault } = useNavigation()
  const handleNavigation = useMemoizedFn((conversation: ConversationResponse) => {
    const from = conversation.page_name || 'home'
    const id = conversation.id
    dispatch(setActiveConversationId(id))
    const metadata = conversation.metadata || '{}'
    if (from === 'home') {
      dispatch(toggleHomeAiAssistant(true))
      navigateToMainTab('home', '/')
    } else if (from === 'creation') {
      dispatch(toggleCreationAiAssistant(true))
      try {
        const parsedMetadata = JSON.parse(metadata)
        const document_id = parsedMetadata.document_id
        navigateToVault(document_id)
      } catch {
        // metadata 可能不是合法 JSON：解析失败就不跳转
      }
    }
  })
  return (
    <CardLayout title={t('home.recentChat')} emptyText={t('home.recentChat.empty')} isEmpty={isEmpty(conversationList)}>
      {(conversationList || [])?.map((conversation) => (
        <div
          key={conversation.id}
          className="flex items-center cursor-pointer justify-between group w-full hover:bg-[var(--color-bg-1)] rounded-[6px] py-[5px] px-[4px]">
          <div
            className=" flex items-center gap-[6px]  "
            key={conversation.id}
            onClick={() => handleNavigation(conversation)}>
            <img src={chatHistoryIcon} className="block" />
            <Typography.Text className="!my-0 !flex-1 !text-[13px] !leading-[22px] !font-normal" ellipsis={{ rows: 1 }}>
              {conversation.title || t('home.untitledConversation')}
            </Typography.Text>
            {conversation.updated_at && (
              <span className="flex text-[var(--color-text-4)]  items-center text-[11px] font-normal leading-[22px] opacity-0 group-hover:opacity-100">
                {formatRelativeTime(conversation.updated_at)}
              </span>
            )}
          </div>
          <div className="flex items-center">
            {/* View button - hidden by default, shown on hover */}
            <button className="opacity-0 group-hover:opacity-100 transition-opacity duration-200 text-[var(--mc-brand-soft)] font-pingfang-sc text-[12px] font-medium leading-[20px] tracking-[0.036px] cursor-pointer">
              {t('home.view')}
            </button>
          </div>
        </div>
      ))}
    </CardLayout>
  )
}
export { ChatCard }
