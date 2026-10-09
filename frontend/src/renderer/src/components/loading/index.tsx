// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import { Button, Progress, Typography } from '@arco-design/web-react'
import { useI18n } from '@renderer/i18n'
import { useEffect, useState } from 'react'
const { Title, Text } = Typography
import logo from '/src/assets/images/logo.png'

export type BackendStatus = 'starting' | 'running' | 'stopped' | 'error'

const LoadingComponent = ({
  backendStatus,
  onRetry,
  onContinue
}: {
  backendStatus: BackendStatus
  /** 后端不可用时的重试；不传则不渲染按钮（避免死按钮）。 */
  onRetry?: () => void
  /** 跳过等待、进入设置继续配置（文案已承诺此出口，不能只剩重试）。 */
  onContinue?: () => void
}) => {
  const { t } = useI18n()
  const [progress, setProgress] = useState(0)
  const [startTime, setStartTime] = useState<number | null>(null)
  const isError = backendStatus === 'error'

  // Calculate target progress based on backend status
  const getProgressByStatus = (status: BackendStatus): number => {
    switch (status) {
      case 'stopped':
        return 10 // Initial state
      case 'starting':
        return 99 // Up to 99% while starting
      case 'running':
        return 100 // Complete
      case 'error':
        return 0 // Error state
      default:
        return 0
    }
  }

  useEffect(() => {
    if (backendStatus === 'starting' && startTime === null) {
      setStartTime(Date.now())
    }

    const targetProgress = getProgressByStatus(backendStatus)

    const interval = setInterval(() => {
      setProgress((prev) => {
        if (backendStatus === 'starting') {
          // Advance based on time, up to 99% within 20s
          const elapsedTime = startTime ? Date.now() - startTime : 0
          const timeProgress = Math.min(elapsedTime / 20000, 1) // 0 ~ 1
          const dynamicTarget = Math.min(10 + timeProgress * 89, 99) // Smoothly from 10 to 99

          return prev < dynamicTarget ? prev + 1 : prev
        }

        if (backendStatus === 'running') {
          // Push directly to 100 in running state
          return prev < 100 ? prev + 1 : 100
        }

        return targetProgress
      })
    }, 200)

    return () => clearInterval(interval)
  }, [backendStatus, startTime])

  return (
    <div
      className="flex flex-col justify-center items-center h-screen text-black"
      style={{
        background:
          'linear-gradient(165.9deg, rgb(209, 192, 211) -3.95%, rgb(217, 218, 233) 3.32%, rgb(242, 242, 242) 23.35%, rgb(253, 252, 248) 71.67%, rgb(249, 250, 236) 76.64%, rgb(255, 236, 221) 83.97%)'
      }}>
      <div style={{ appRegion: 'drag' } as React.CSSProperties} className="absolute top-0 left-0 w-full h-[30px]" />
      <img src={logo} alt="Logo" className="w-[100px] h-[100px]" />
      <Title className="text-white text-[32px] font-bold" style={{ marginBottom: '40px', marginTop: '24px' }}>
        {t('common.welcome')}
      </Title>

      {!isError ? (
        <Progress
          percent={progress}
          width={400}
          color={'var(--color-text-1)'}
          animation={backendStatus === 'starting' || backendStatus === 'running'}
          showText={true}
          formatText={(percent) => `${Math.round(percent || 0)}%`}
        />
      ) : null}

      <Text className="text-[var(--color-text-2)] text-14" style={{ marginTop: '16px' }}>
        {isError ? t('common.backendUnavailable') : t('common.startingHint')}
      </Text>

      {isError && (onRetry || onContinue) ? (
        <div
          className="flex flex-wrap items-center justify-center gap-3"
          style={{ marginTop: '24px', appRegion: 'no-drag' } as React.CSSProperties}>
          {onRetry ? (
            <Button
              type="primary"
              data-testid="backend-retry"
              onClick={onRetry}
              className="!bg-[rgb(var(--primary-6))] !border-[rgb(var(--primary-6))]">
              {t('common.retry')}
            </Button>
          ) : null}
          {onContinue ? (
            <Button type="secondary" data-testid="backend-continue-settings" onClick={onContinue}>
              {t('common.continueInSettings')}
            </Button>
          ) : null}
        </div>
      ) : null}
    </div>
  )
}

export default LoadingComponent
