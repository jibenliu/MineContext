import { Image, Tooltip } from '@arco-design/web-react'
import { useI18n } from '@renderer/i18n'
import React from 'react'

import { ScreenshotImage } from './screenshot-image'

export interface RecordingStats {
  /** 采到的张数 */
  captured_screenshots: number
  /** 其中分析出结果的张数（没配模型时为 0） */
  processed_screenshots: number
  failed_screenshots: number
  generated_activities: number
  next_activity_eta_seconds: number
  recent_errors: Array<{
    error_message: string
    processor_name: string
    timestamp: string
  }>
  recent_screenshots: string[]
}

interface RecordingStatsCardProps {
  stats: RecordingStats | null
}

const RecordingStatsCard: React.FC<RecordingStatsCardProps> = ({ stats }) => {
  const { t } = useI18n()

  if (!stats) {
    return null
  }

  return (
    <div className="mt-2">
      {/* Recent screenshots display */}
      {stats.recent_screenshots && stats.recent_screenshots.length > 0 && (
        <div className="mb-2 flex flex-wrap gap-2">
          <Image.PreviewGroup infinite className="[&_.arco-image-preview-img]:!scale-80">
            {stats.recent_screenshots.map((path, index) => (
              <ScreenshotImage key={path} path={path} alt={`screenshot-${index + 1}`} index={index} />
            ))}
          </Image.PreviewGroup>
        </div>
      )}

      {/* Stats text：采到多少张、其中多少张分析出了结果 —— 两件事分开说，
          合成一个数会把「采到了」讲成「处理完了」 */}
      <div className="text-xs text-[var(--color-text-3)]">
        <span className="text-[#00B42A] font-medium">{stats.captured_screenshots}</span>{' '}
        <span>
          {t(
            stats.captured_screenshots === 1 ? 'screenMonitor.stats.capturedOne' : 'screenMonitor.stats.capturedMany'
          )}
        </span>
        <span className="mx-2">•</span>
        <span className="text-[var(--color-text-2)] font-medium">{stats.processed_screenshots}</span>{' '}
        <span>
          {t(
            stats.processed_screenshots === 1 ? 'screenMonitor.stats.processedOne' : 'screenMonitor.stats.processedMany'
          )}
        </span>
        {stats.failed_screenshots > 0 && (
          <>
            <span className="mx-2">•</span>
            <Tooltip
              content={
                <div className="max-w-xs">
                  <div className="font-medium mb-1">{t('screenMonitor.stats.recentErrors')}</div>
                  {stats.recent_errors.length > 0 ? (
                    <ul className="text-xs space-y-1">
                      {stats.recent_errors.map((error, index) => (
                        <li key={index} className="break-words">
                          {error.error_message}
                        </li>
                      ))}
                    </ul>
                  ) : (
                    <span className="text-xs">{t('screenMonitor.stats.noErrorDetail')}</span>
                  )}
                </div>
              }>
              <span className="text-[rgb(var(--danger-6))] font-medium cursor-help underline decoration-dashed">
                {t(
                  stats.failed_screenshots === 1 ? 'screenMonitor.stats.failedOne' : 'screenMonitor.stats.failedMany',
                  { count: stats.failed_screenshots }
                )}
              </span>
            </Tooltip>
          </>
        )}
      </div>
    </div>
  )
}

export default RecordingStatsCard
