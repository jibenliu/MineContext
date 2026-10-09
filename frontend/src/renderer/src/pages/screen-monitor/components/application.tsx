// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import { Button } from '@arco-design/web-react'
import screenIcon from '@renderer/assets/icons/screen.svg'
import { useI18n } from '@renderer/i18n'
import clsx from 'clsx'
import { FC } from 'react'
export interface ApplicationProps {
  value?: any[]
  onCancel?: () => void
  visible?: boolean
  onOk?: () => void
}

const Application: FC<ApplicationProps> = (props) => {
  const { value: source = [], onCancel, visible, onOk } = props
  const { t } = useI18n()
  return (
    <div className="flex justify-between items-center">
      {source.length > 0 ? (
        <div className="flex items-center flex-1">
          <div className="flex items-center relative flex-1 h-[32px]">
            {source.slice(0, 9).map((item, index) => {
              return (
                <div
                  key={item.id}
                  className={clsx(
                    'w-[32px] h-[32px] rounded-[32px] flex items-center justify-center bg-[var(--color-bg-2)] border border-solid border-[var(--color-border-1)]',
                    'absolute top-0',
                    index === 0 ? `left-0` : ``
                  )}
                  style={{ left: `${index * 20}px` }}>
                  <div className="w-[28px] h-[28px] rounded-[28px] flex items-center justify-center bg-[var(--color-fill-2)]">
                    {item.type === 'window' ? (
                      item.appIcon || item.thumbnail ? (
                        <img
                          src={item.appIcon || item.thumbnail || ''}
                          alt={item.name || ''}
                          className="w-[18px] h-[18px]"
                        />
                      ) : (
                        <span className="w-[18px] h-[18px] rounded-[4px] bg-[var(--color-fill-2)]" />
                      )
                    ) : null}
                    {item.type === 'screen' ? <img src={screenIcon} alt="" className="w-[16px] h-[16px]" /> : null}
                  </div>
                </div>
              )
            })}
          </div>
          {source.length > 9 ? (
            <div className="text-[12px] leading-[16px] text-[var(--color-text-2)]">+{source.length - 9}</div>
          ) : null}
        </div>
      ) : null}
      {!visible ? (
        <Button type="text" className="!px-0 ml-[24px]" onClick={onOk}>
          {t('common.select')}
        </Button>
      ) : (
        <Button type="text" className="!px-0 ml-[24px]" onClick={onCancel}>
          {t('common.close')}
        </Button>
      )}
    </div>
  )
}
export { Application }
