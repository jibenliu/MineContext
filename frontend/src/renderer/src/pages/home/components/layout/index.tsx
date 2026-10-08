// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import { Card } from '@arco-design/web-react'
import { useI18n } from '@renderer/i18n'
import { FC } from 'react'

interface LatestActivityCardProps {
  title: string
  emptyText?: string
  children?: React.ReactNode
  seeAllClick?: () => void
  isEmpty?: boolean
}

const CardLayout: FC<LatestActivityCardProps> = (props) => {
  const { title, seeAllClick, emptyText, children, isEmpty } = props
  const { t } = useI18n()

  return (
    <Card
      headerStyle={{ width: '100%' }}
      bodyStyle={{ width: '100%', overflowY: 'auto', scrollbarWidth: 'none', flex: '1' }}
      title={
        <div className="flex items-center gap-2 self-stretch">
          <div className="flex-1 text-[var(--color-text-1)] font-['Roboto'] text-sm font-medium leading-[22px]">
            {title}
          </div>
          {seeAllClick ? (
            <div
              className="text-[var(--color-text-2)] font-['PingFang SC'] text-xs leading-[20px] cursor-pointer font-medium"
              onClick={seeAllClick}>
              {t('home.seeAll')}
            </div>
          ) : null}
        </div>
      }
      className="flex w-full h-[160px] p-3 flex-col items-start gap-3 self-stretch rounded-[10px] border border-[rgba(225,227,239,0.80)] bg-white">
      <div
        className={`flex flex-col items-center gap-1 flex-1 self-stretch ${isEmpty ? '' : 'justify-start items-start'} h-full`}>
        {!isEmpty ? (
          children
        ) : (
          <div className="flex w-[340px] h-full flex-col justify-center items-center text-[var(--color-text-3)] text-center font-['Roboto'] text-[13px] font-normal leading-[22px] tracking-[0.039px]">
            {emptyText}
          </div>
        )}
      </div>
    </Card>
  )
}

export { CardLayout }
