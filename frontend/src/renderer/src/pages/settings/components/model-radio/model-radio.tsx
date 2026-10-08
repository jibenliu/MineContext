// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import { ModelInfoList } from '../../constants'
interface ModelRadioProps {
  value?: string
  onChange?: (v: string) => void
}

const ModelRadio = ({ value, onChange }: ModelRadioProps) => {
  return (
    // 宽度由内容决定：三个 40px 的圆加两个 16px 间距需要 152px，
    // 写死一个更小的固定值会让子项溢出容器、标签互相压住。
    <div className="w-fit flex items-center gap-[16px]">
      {ModelInfoList?.map((item) => {
        return (
          <div key={item.value} className="w-10  cursor-pointer">
            <div
              className={`w-10 h-10 rounded-full border flex items-center justify-center overflow-hidden ${item.value === value ? 'border-[var(--mc-brand)]' : 'border-[var(--color-border-2)]'} ${item.value === value ? 'border-2' : 'border-1'}`}
              onClick={() => {
                onChange?.(item.value)
              }}>
              <div>
                <div className="w-full h-full flex items-center justify-center">{item.icon}</div>
              </div>
            </div>
            {/* 高度与行高一起给：只给 height 的话行高继承到 15px，10px 的字会被裁掉底部 */}
            <div className="w-10 h-4 mt-[4px] flex items-center justify-center overflow-hidden text-[var(--color-text-2)] text-[10px] leading-4">
              {item.key}
            </div>
          </div>
        )
      })}
    </div>
  )
}

export default ModelRadio
