// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import { Button, Modal } from '@arco-design/web-react'
import { useI18n } from '@renderer/i18n'
import { FC, ReactNode } from 'react'

import { MarkdownContent } from '../ai-assistant'
import titleBg from './assets/bg.png'
interface ProactiveFeedModalProps {
  visible: boolean
  onCancel: () => void
  content?: ReactNode
  time?: ReactNode
}

const ProactiveFeedModal: FC<ProactiveFeedModalProps> = (props) => {
  const { visible, onCancel, content } = props
  const { t } = useI18n()

  // Modal header (including icon, title, subtitle)
  const modalHeader = (
    // 背景图只走内联样式：把运行时变量拼进 Tailwind 的任意值类，构建期解析不了
    // 那个占位符（会报一条无法解析的告警），实际也不生效。
    <div
      className="!bg-no-repeat w-full h-[76px] !bg-cover !bg-center rounded-[8px] overflow-hidden flex items-center"
      style={{ background: `url(${titleBg})` }}>
      <div className="ml-[118px]">
        <div
          className="text-[20px] leading-[22px] bg-[linear-gradient(271.9deg,_#C296FF_-23.68%,_#FF875F_100.99%)]
  bg-clip-text text-transparent font-bold">
          {t('common.proactiveFeed')}
        </div>
        <div className="text-[12px] leading-[20px] text-[var(--color-text-3)]">{t('common.proactiveFeedHint')}</div>
      </div>
    </div>
  )

  // Modal content area (tutorial text)
  const modalContent = (
    <div className="mt-4 h-full overflow-x-hidden overflow-y-auto">
      <MarkdownContent content={String(content || ``)} />
    </div>
  )

  // Modal footer (confirmation button)
  const modalFooter = (
    <div className="flex justify-end p-4">
      <Button
        type="primary"
        onClick={onCancel}
        className="flex w-[116px] px-4 py-[5px] justify-center items-center gap-2 rounded-md"
        style={{
          backgroundColor: 'rgb(var(--primary-6))'
        }}>
        {t('common.gotIt')}
      </Button>
    </div>
  )

  return (
    <Modal
      visible={visible}
      title={modalHeader}
      footer={modalFooter}
      onCancel={onCancel}
      className="!w-[700px] !h-[588px] [&_.arco-modal-header]:!h-auto [&_.arco-modal-header]:!px-0 px-[20px] pt-[16px] pb-[20px] [&_>_div]:nth-2:flex [&_>_div]:nth-2:flex-col [&_>_div]:nth-2:h-full [&_.arco-modal-content]:!flex-1 [&_.arco-modal-content]:!p-0 [&_.arco-modal-footer]:!p-0 [&_.arco-modal-content]:!overflow-y-hidden [&_.arco-modal-footer]:!h-auto"
      closable={false} // Hide the default "Close" button, only keep the custom "I got it"
    >
      {modalContent}
    </Modal>
  )
}

export { ProactiveFeedModal }
