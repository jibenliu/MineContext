// 设置页：界面语言切换。语言状态在 redux（`setting.locale`）里，随 store 持久化。
//
// 为什么用 `Segmented` 而不是下拉：只有两种语言，两个选项直接摆出来比「点开再看」
// 少一次交互，也避免用户以为「这里只是个只读展示」。

import { Radio, Typography } from '@arco-design/web-react'
import { type Locale, LOCALES, useI18n } from '@renderer/i18n'
import { setLocale } from '@renderer/store/setting'
import { useDispatch } from 'react-redux'

const { Text } = Typography

export function LanguageSwitch() {
  const { t, locale } = useI18n()
  const dispatch = useDispatch()

  return (
    <div className="mt-[10px] flex max-w-[520px] items-center justify-between gap-6 py-1">
      <div className="flex flex-col">
        <span className="text-[14px]">{t('app.language')}</span>
        <Text type="secondary" className="!text-[12px]">
          {t('app.language.hint')}
        </Text>
      </div>
      <Radio.Group
        type="button"
        size="small"
        value={locale}
        onChange={(value) => dispatch(setLocale(value as Locale))}
        aria-label={t('app.language')}>
        {LOCALES.map((item) => (
          <Radio key={item.value} value={item.value}>
            {item.label}
          </Radio>
        ))}
      </Radio.Group>
    </div>
  )
}
