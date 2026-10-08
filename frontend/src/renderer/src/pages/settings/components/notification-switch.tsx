import { Switch, Typography } from '@arco-design/web-react'
import { useI18n } from '@renderer/i18n'
import store from '@renderer/store'
import { setSystemNotificationsEnabled } from '@renderer/store/setting'
import { useSyncExternalStore } from 'react'

export function NotificationSwitch() {
  const { t } = useI18n()
  const enabled = useSyncExternalStore(
    store.subscribe,
    () => store.getState().setting.systemNotificationsEnabled !== false
  )
  return (
    <div className="flex max-w-[520px] items-center justify-between gap-6 py-1">
      <div className="flex flex-col">
        <span className="text-[14px]">{t('settings.notifications')}</span>
        <Typography.Text type="secondary" className="!text-[12px]">
          {t('settings.notifications.hint')}
        </Typography.Text>
      </div>
      <Switch
        aria-label={t('settings.notifications')}
        checked={enabled}
        onChange={(next) => store.dispatch(setSystemNotificationsEnabled(next))}
      />
    </div>
  )
}
