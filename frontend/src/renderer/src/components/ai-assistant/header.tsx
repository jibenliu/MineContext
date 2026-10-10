import { Button, Divider, Popover, Space } from '@arco-design/web-react'
import { IconClose } from '@arco-design/web-react/icon'
import addChatIcon from '@renderer/assets/icons/ai-assistant/add-chat.svg'
import chatHistoryIcon from '@renderer/assets/icons/ai-assistant/chat-history.svg'
import { conversationService } from '@renderer/services/conversation-service'
import { readActiveVaultId } from '@renderer/utils/vault'
import { useMemoizedFn, useRequest } from 'ahooks'
import clsx from 'clsx'
import { FC, MouseEvent, useState } from 'react'

import { ChatHistoryList } from './chat-history-list'
export interface AIAssistantHeaderProps {
  onClose: () => void
  startNewConversation: () => void
  conversationId: number
  handleGetMessages?: (conversationId: number) => void
  pageName: string
}
const AIAssistantHeader: FC<AIAssistantHeaderProps> = (props) => {
  const { onClose, startNewConversation, handleGetMessages, pageName } = props
  const {
    runAsync: getConversationList,
    data: conversationList,
    refresh: refreshConversationList
  } = useRequest(
    async () => {
      const res = await conversationService.getConversationList({
        limit: 30,
        offset: 0,
        page_name: pageName,
        status: 'active',
        vault_id: readActiveVaultId()
      })
      return res.items || []
    },
    { manual: true }
  )
  const handleVisibleChange = useMemoizedFn((visible: boolean) => {
    if (visible) {
      getConversationList()
    }
  })
  const [popupVisible, setPopupVisible] = useState(false)
  const showChatHistoryPopup = useMemoizedFn((e: MouseEvent) => {
    e?.stopPropagation()
    e?.preventDefault()
    setPopupVisible(true)
  })
  const hideChatHistoryPopup = useMemoizedFn((e?: MouseEvent) => {
    e?.stopPropagation()
    e?.preventDefault()
    setPopupVisible(false)
  })
  const selectConversation = useMemoizedFn((e: MouseEvent, conversationId: number) => {
    hideChatHistoryPopup(e)
    handleGetMessages?.(conversationId)
  })
  return (
    <div className="flex items-center justify-between px-[16px] border-b border-[var(--color-fill-2)]">
      <div
        className="flex items-center font-medium text-[var(--mc-brand)] cursor-pointer"
        onClick={startNewConversation}
        style={{ padding: '12px 16px 12px 0' }}>
        <img src={addChatIcon} alt="add-chat" className="w-[16px] h-[16px] mr-[6px]" />
        New chat
      </div>
      <Space>
        <Popover
          position="br"
          content={
            <ChatHistoryList
              conversationList={conversationList || []}
              handleGetMessages={selectConversation}
              onConversationUpdate={getConversationList}
              refreshConversationList={refreshConversationList}
            />
          }
          trigger={['click']}
          onVisibleChange={handleVisibleChange}
          popupVisible={popupVisible}
          className="[&_.arco-popover-content]:!p-0"
          triggerProps={{
            popupAlign: {
              top: 8
            },
            onClickOutside: hideChatHistoryPopup
          }}>
          <div
            className={clsx(
              'flex items-center justify-center w-[32px] h-[32px] hover:bg-[var(--color-bg-1)] rounded-[6px] cursor-pointer',
              { 'bg-[var(--color-bg-1)]': popupVisible }
            )}
            onClick={showChatHistoryPopup}>
            <img src={chatHistoryIcon} className="w-[14px] h-[14px]" />
          </div>
        </Popover>

        <Divider type="vertical" className="!mx-0" />
        <Button
          type="text"
          icon={<IconClose className="!text-[var(--color-text-3)]" />}
          onClick={onClose}
          className="text-[var(--color-text-3)]"
        />
      </Space>
    </div>
  )
}
export { AIAssistantHeader }
