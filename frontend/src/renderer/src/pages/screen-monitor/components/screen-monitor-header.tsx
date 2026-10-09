import { Button, Popover, Space, Typography } from '@arco-design/web-react'
import { IconPlayArrow, IconRecordStop, IconSettings } from '@arco-design/web-react/icon'
import { useI18n } from '@renderer/i18n'
import React from 'react'

const { Title, Text } = Typography

interface ScreenMonitorHeaderProps {
  hasPermission: boolean
  isMonitoring: boolean
  isToday: boolean
  screenAllSources: any[]
  appAllSources: any[]
  onOpenSettings: () => void
  onStartMonitoring: () => void
  onStopMonitoring: () => void
  onRequestPermission: () => void
}

const ScreenMonitorHeader: React.FC<ScreenMonitorHeaderProps> = ({
  hasPermission,
  isMonitoring,
  isToday,
  screenAllSources,
  appAllSources,
  onOpenSettings,
  onStartMonitoring,
  onStopMonitoring
}) => {
  const { t } = useI18n()
  return (
    <div className="flex justify-between items-start mb-3 flex-col md:flex-row">
      <div className="w-full md:w-4/5">
        <Title
          heading={3}
          className="[&_.arco-typography]: !mt-1 [&_.arco-typography]: !font-bold [&_.arco-typography]: !text-[24px] [&_.arco-typography]: !text-[var(--color-text-1)]">
          {t('screenMonitor.title')}
        </Title>
        <Text type="secondary" className="[&_.arco-typography]: !text-[13px]">
          {t('screenMonitor.subtitle')}
        </Text>
      </div>
      <div className="flex items-center ml-0 md:ml-6 mt-4 md:mt-0 justify-end">
        {hasPermission ? (
          <Space>
            <Popover content={t('screenMonitor.settingsDisabledHint')} disabled={!isMonitoring}>
              <Button
                type="outline"
                icon={<IconSettings />}
                size="large"
                disabled={isMonitoring}
                onClick={onOpenSettings}
                className="mc-secondary-btn !bg-white !border-[#c9cdd4] !text-[var(--color-text-1)] !font-medium hover:!bg-[#f7f8fa] hover:!border-[#c9cdd4]">
                {t('common.settings')}
              </Button>
            </Popover>
            {!isMonitoring ? (
              <Popover
                content={t('screenMonitor.selectSourceHint')}
                disabled={!(screenAllSources.length === 0 && appAllSources.length === 0)}>
                <Button
                  type="primary"
                  icon={<IconPlayArrow />}
                  size="large"
                  onClick={onStartMonitoring}
                  disabled={isMonitoring || !isToday}
                  style={{
                    background: 'rgb(var(--primary-6))'
                  }}>
                  {t('screenMonitor.startRecording')}
                </Button>
              </Popover>
            ) : (
              <Button
                type="primary"
                status="danger"
                icon={<IconRecordStop />}
                size="large"
                onClick={onStopMonitoring}
                className="[&_.arco-btn-primary]: !bg-[rgb(var(--danger-6))] [&_.arco-btn-primary:hover]: !bg-[rgb(var(--danger-6))]">
                {t('screenMonitor.stopRecording')}
              </Button>
            )}
          </Space>
        ) : null}
      </div>
    </div>
  )
}

export default ScreenMonitorHeader
