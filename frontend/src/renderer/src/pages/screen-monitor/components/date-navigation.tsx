import { Button, DatePicker, Typography } from '@arco-design/web-react'
import { IconDown, IconLeft, IconRight } from '@arco-design/web-react/icon'
import { useI18n } from '@renderer/i18n'
import dayjs from 'dayjs'
import React from 'react'

const { Text } = Typography

interface DateNavigationProps {
  hasPermission: boolean
  currentDate: Date
  isToday: boolean
  onPreviousDay: () => void
  onNextDay: () => void
  onDateChange: (dateString: any, date: any) => void
  onSetCurrentDate: (date: Date) => void
  disabledDate: (current: any) => boolean
}

const DateNavigation: React.FC<DateNavigationProps> = ({
  hasPermission,
  currentDate,
  isToday,
  onPreviousDay,
  onNextDay,
  onDateChange,
  onSetCurrentDate,
  disabledDate
}) => {
  const { t } = useI18n()
  return (
    <div className="flex justify-between items-center mb-4">
      <div className="flex items-center">
        {hasPermission && (
          <>
            <Button
              className="[&_.arco-btn-primary]: !bg-[var(--color-bg-2)] [&_.arco-btn-primary]: !border-[var(--color-border-2)] [&_.arco-btn-primary]: !h-6 [&_.arco-btn-primary]: !text-[var(--color-text-1)] [&_.arco-btn-primary]:  !text-xs [&_.arco-btn-primary]: !mr-2 [&_.arco-btn:hover]: !bg-[var(--color-fill-1)]"
              onClick={() => onSetCurrentDate(new Date())}>
              {t('screenMonitor.today')}
            </Button>
            <Button
              icon={<IconLeft />}
              onClick={onPreviousDay}
              className="[&_.arco-btn-primary]: !mr-3 [&_.arco-btn-primary]: !bg-transparent [&_.arco-btn-primary]: !border-none [&_.arco-btn:hover]: !bg-[var(--color-fill-2)]"
            />
            <DatePicker
              value={currentDate}
              onChange={onDateChange}
              disabledDate={disabledDate}
              triggerElement={
                <Button className="[&_.arco-btn-primary]: !h-[22px] [&_.arco-btn-primary]: !bg-transparent [&_.arco-btn-primary]: !border-none [&_.arco-btn-primary]: !p-0 [&_.arco-btn:hover]: !bg-[var(--color-fill-1)]">
                  <Text className="[&_.arco-typography]: !font-medium [&_.arco-typography]: !text-sm">
                    {dayjs(currentDate).format('MMMM D, YYYY')}
                  </Text>
                  <IconDown className="ml-1 w-3 h-3" />
                </Button>
              }
            />
            <Button
              icon={<IconRight />}
              onClick={onNextDay}
              className="[&_.arco-btn-primary]: !ml-3 [&_.arco-btn-primary]: !bg-transparent [&_.arco-btn-primary]: !border-none [&_.arco-btn:hover]: !bg-[var(--color-fill-2)]"
              disabled={isToday}
            />
          </>
        )}
      </div>
    </div>
  )
}

export default DateNavigation
