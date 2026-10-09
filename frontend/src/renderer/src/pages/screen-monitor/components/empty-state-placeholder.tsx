import { Button, Typography } from '@arco-design/web-react'
import NeedPermission from '@renderer/assets/images/screen-monitor/need-permission.svg'
import screenMonitorEmpty from '@renderer/assets/images/screen-monitor/screen-monitor-empty.svg'
import Stopped from '@renderer/assets/images/screen-monitor/stopped.png'
import { useI18n } from '@renderer/i18n'
import React from 'react'

import { SCREEN_INTERVAL_TIME } from '../constant'

const { Text } = Typography

interface EmptyStatePlaceholderProps {
  hasPermission: boolean
  isToday: boolean
  onGrantPermission: () => void
}

const EmptyStatePlaceholder: React.FC<EmptyStatePlaceholderProps> = ({ hasPermission, isToday, onGrantPermission }) => {
  const { t } = useI18n()
  return (
    <div className="flex items-center justify-center flex-1 min-h-[300px]">
      <div className="text-center flex flex-col items-center justify-center">
        {hasPermission ? (
          isToday ? (
            <>
              <div className="flex h-[96px] w-[96px] items-center justify-center rounded-[12px] border border-[var(--color-border-2)] bg-white">
                <img src={Stopped} alt="" style={{ width: 66, height: 78 }} />
              </div>
              <Text style={{ marginTop: 16, width: 270, color: 'var(--color-text-2)', fontSize: 12 }}>
                {t('screenMonitor.empty.startHint', { minutes: SCREEN_INTERVAL_TIME })}
              </Text>
            </>
          ) : (
            <>
              <div className="flex h-[96px] w-[96px] items-center justify-center rounded-[12px] border border-[var(--color-border-2)] bg-white">
                <img src={screenMonitorEmpty} alt="" style={{ width: 66, height: 78 }} />
              </div>
              <Text style={{ marginTop: 16, width: 270, color: 'var(--color-text-2)', fontSize: 12 }}>
                {t('screenMonitor.empty.noData')}
              </Text>
            </>
          )
        ) : (
          <>
            <img src={NeedPermission} alt="Need permission" style={{ width: 286, height: 168, marginLeft: 67 }} />
            <Text style={{ marginTop: 16, width: 440, color: 'var(--color-text-2)', fontSize: 12 }}>
              {t('screenMonitor.empty.permissionHint', { minutes: SCREEN_INTERVAL_TIME })}
            </Text>
            <Button
              type="primary"
              size="large"
              onClick={onGrantPermission}
              className="[&_.arco-btn-primary]: !mt-6 [&_.arco-btn-primary]: !font-medium [&_.arco-btn-primary]: !bg-[rgb(var(--primary-6))] !border-[rgb(var(--primary-6))]">
              {t('screenMonitor.empty.enablePermission')}
            </Button>
          </>
        )}
      </div>
    </div>
  )
}

export default EmptyStatePlaceholder
