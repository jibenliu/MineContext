import { Alert, Button, Form, Image, Message, Modal } from '@arco-design/web-react'
import { CaptureSource } from '@interface/common/source'
import { useObservableTask } from '@renderer/atom/event-loop.atom'
import { useScreen } from '@renderer/hooks/use-screen'
import { useSetting } from '@renderer/hooks/use-setting'
import { useI18n } from '@renderer/i18n'
import { useAppDispatch, useAppSelector } from '@renderer/store'
import { refreshCaptureSources, refreshCaptureSourcesFromSettings } from '@renderer/store/capture-sources'
import { withParsedResources } from '@renderer/utils/resources'
import { getLogger } from '@shared/logger/renderer'
import { useMemoizedFn, useMount } from 'ahooks'
import dayjs from 'dayjs'
import { get } from 'lodash'
import React, { useEffect, useMemo, useRef, useState } from 'react'
import { useLocation, useNavigate } from 'react-router-dom'

import DateNavigation from './components/date-navigation'
import EmptyStatePlaceholder from './components/empty-state-placeholder'
import { categoryFromMetadata } from './components/group-activity-timeline'
import type { RecordingStats } from './components/recording-stats-card'
import RecordingTimeline from './components/recording-timeline'
// Extracted components
import ScreenMonitorHeader from './components/screen-monitor-header'
import SettingsModal from './components/settings-modal'

const logger = getLogger('ScreenMonitor')

export interface Activity {
  id: string
  start_time: string
  end_time: string
  resources: Array<{
    type: string
    id: string
    path: string
  }>
  title: string
  content: string
  /** 投影分类；兼容面在 metadata 里，扩展面在 v1.category */
  category?: string | null
  metadata?: string
}

const ScreenMonitor: React.FC = () => {
  const { t } = useI18n()
  const location = useLocation()
  const navigate = useNavigate()
  const {
    recordInterval,
    recordingHours,
    enableRecordingHours,
    applyToDays,
    setRecordInterval,
    setEnableRecordingHours,
    setRecordingHours,
    setApplyToDays
  } = useSetting()
  const {
    currentSession,
    hasPermission = false,
    grantPermission,
    selectedImage,
    setSelectedImage,
    getNewActivities,
    getActivitiesByDate
  } = useScreen()
  const [isMonitoring, setIsMonitoring] = useState(false)
  useMount(() => {
    window.serverPushAPI.pushScreenMonitorStatus((status) => {
      setIsMonitoring(status === 'running')
    })
  })
  // Get selectable sources
  const dispatch = useAppDispatch()
  const sources = useAppSelector((state) => state.captureSources.available)
  useEffect(() => {
    const available = dispatch(refreshCaptureSources())
    const saved = dispatch(refreshCaptureSourcesFromSettings())
    return () => {
      available.abort()
      saved.abort()
    }
  }, [dispatch])
  // Used to update whether the optional application list has been read to render the page
  // const [sourcesRead, setSourcesRead] = useState(false)
  const screenAllSources = useMemo(() => {
    return (sources.state === 'hasData' ? sources.data.screenSources : []).filter((v) => v.isVisible)
  }, [sources])
  const appAllSources = useMemo(() => {
    return (sources.state === 'hasData' ? sources.data.appSources : []).filter((v) => v.isVisible)
  }, [sources])

  const [currentDate, setCurrentDate] = useState(dayjs().toDate())
  const isToday = dayjs(currentDate).isSame(dayjs(), 'day')
  const screenshots = currentSession?.screenshots || {}
  const [settingsVisible, setSettingsVisible] = useState(false)
  const [activities, setActivities] = useState<Activity[]>([])
  const [recordingStats, setRecordingStats] = useState<RecordingStats | null>(null)
  const activityPollingRef = useRef<NodeJS.Timeout | null>(null)
  const statsPollingRef = useRef<NodeJS.Timeout | null>(null)
  // 轮询水位线：只问「比它更新的活动」。首次取当前时刻，之后由轮询按后端返回的
  // **升序批次**推进（见下方 `uniqueActivities` / `filteredActivities` 的用法）。
  // 这里不要从 `activities` 里取最后一条当起点：本组件的合并是把新批次**前插**
  // （`[...新, ...旧]`），数组整体不是时间序，取「最后一条」拿到的是最旧那条。
  const lastCheckedTimeRef = useRef<string>(dayjs().toISOString())
  const isScreenLockedRef = useRef(false)

  // Settings form state
  const [tempRecordInterval, setTempRecordInterval] = useState(recordInterval)
  const [tempEnableRecordingHours, setTempEnableRecordingHours] = useState(enableRecordingHours)
  const [tempRecordingHours, setTempRecordingHours] = useState<[string, string]>(recordingHours as [string, string])
  const [tempApplyToDays, setTempApplyToDays] = useState(applyToDays)

  // Refresh the application list and trigger a re-render
  const refreshSourcesRead = useMemoizedFn(async () => {
    await dispatch(refreshCaptureSources()).unwrap()
  })

  useEffect(() => {
    const initActivities = async () => {
      const date = dayjs(currentDate).startOf('day').toDate()
      const todayActivities = await getActivitiesByDate(date)
      const todayActivitiesParsed: Activity[] = todayActivities.map((item: any) => ({
        ...item,
        resources: withParsedResources(item).resources,
        category: item.category ?? categoryFromMetadata(item.metadata)
      }))
      const uniqueActivities = Array.from(new Map(todayActivitiesParsed.map((item) => [item.id, item])).values())
      setActivities(uniqueActivities)

      // Reset lastCheckedTimeRef to the time of the last activity of the day
      if (uniqueActivities.length > 0) {
        const latestActivity = uniqueActivities[uniqueActivities.length - 1]
        lastCheckedTimeRef.current = latestActivity.end_time || latestActivity.start_time
      } else {
        // If there are no activities, reset to the start of the day
        lastCheckedTimeRef.current = dayjs(currentDate).startOf('day').toISOString()
      }
    }
    initActivities()
  }, [currentDate, getActivitiesByDate])

  // Manage polling when date or monitoring status changes
  useEffect(() => {
    if (isMonitoring) {
      if (isToday) {
        // If switched to today and monitoring, start polling
        startActivityPolling()
        startStatsPolling()
      } else {
        // If switched to historical date, stop polling
        stopActivityPolling()
        stopStatsPolling()
      }
    } else {
      // If not monitoring, stop all polling
      stopActivityPolling()
      stopStatsPolling()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentDate, isMonitoring, isToday])

  const handlePreviousDay = () => {
    const newDate = dayjs(currentDate).subtract(1, 'day').toDate()
    setCurrentDate(newDate)
  }

  const handleNextDay = () => {
    const newDate = dayjs(currentDate).add(1, 'day').toDate()
    setCurrentDate(newDate)
  }

  const handleDateChange = (_dateString, date) => {
    setCurrentDate(date.toDate())
  }

  const disabledDate = (current) => {
    return current && dayjs(current).isAfter(dayjs(), 'day')
  }

  // Check if recording is possible under the current settings
  const [canRecord, setCanRecord] = useState(false)
  // 不能录制时把原因一起存下来（后端 /api/capture/status 的 reason），界面直接告诉用户
  const [recordBlockReason, setRecordBlockReason] = useState<string | undefined>(undefined)
  const [captureEnabled, setCaptureEnabled] = useState<boolean | undefined>(undefined)
  const [windowsReason, setWindowsReason] = useState<string | null | undefined>(undefined)
  const [screenRecordingTcc, setScreenRecordingTcc] = useState<boolean | undefined>(undefined)
  const checkCanRecord = useMemoizedFn(async () => {
    const result = await window.screenMonitorAPI.checkCanRecord()
    setCanRecord(result.canRecord)
    setRecordBlockReason(result.reason)
    setCaptureEnabled(result.enabled)
    setWindowsReason(result.windows_reason)
    setScreenRecordingTcc(result.screen_recording_tcc)
    setIsMonitoring(result.status === 'running')
    return result
  })

  // Start monitoring session
  const startMonitoring = useMemoizedFn(async () => {
    try {
      // 权限已开仍可能因无显示器/未挂载采集控制而不可录；先问清楚再 start，避免静默失败。
      const readiness = await checkCanRecord()
      if (!readiness.canRecord) {
        Message.error(readiness.reason || t('screenMonitor.timeline.recordingUnavailableReason'))
        return
      }
      await window.screenMonitorAPI.updateModelConfig({
        recordInterval,
        recordingHours,
        enableRecordingHours,
        applyToDays
      })
      await window.screenMonitorAPI.startTask()
      // 不依赖 SSE：推送丢了时界面会一直停在「开始录制」空态。
      setIsMonitoring(true)
      await checkCanRecord()
      startActivityPolling()
      startStatsPolling()
    } catch (error) {
      const message =
        (error as { message?: string; remediation?: string })?.message ||
        (error as Error)?.message ||
        t('screenMonitor.timeline.recordingUnavailableReason')
      const remediation = (error as { remediation?: string })?.remediation
      Message.error(remediation ? `${message}（${remediation}）` : message)
      logger.error('Failed to start recording', error)
    }
  })

  // Stop monitoring
  const stopMonitoring = useMemoizedFn(async () => {
    try {
      if (isMonitoring) {
        await window.screenMonitorAPI.stopTask()
        setIsMonitoring(false)
        stopActivityPolling()
        stopStatsPolling()
        await checkCanRecord()
      }
    } catch (error) {
      const message = (error as Error)?.message || t('screenMonitor.stopRecording')
      Message.error(message)
      logger.error('Failed to stop recording', error)
    }
  })

  const pauseMonitoring = useMemoizedFn(() => {
    logger.info('Screen locked, pausing monitoring timers')
    stopActivityPolling()
    stopStatsPolling()
  })

  // Resume monitoring (when screen is unlocked)
  const resumeMonitoring = useMemoizedFn(() => {
    if (isMonitoring && !isScreenLockedRef.current) {
      // Resume activity polling
      startActivityPolling()
      // Resume stats polling
      startStatsPolling()
    }
  })

  // Start polling for new activities
  const startActivityPolling = useMemoizedFn(() => {
    if (activityPollingRef.current) {
      clearInterval(activityPollingRef.current)
    }
    // Immediately execute a check for new activities
    const checkNewActivities = async () => {
      try {
        // Only poll for new activities when viewing today
        if (!isToday) {
          return
        }

        const newActivities = await getNewActivities(lastCheckedTimeRef.current)
        const newActivitiesParsed: Activity[] = newActivities.map((item: any) => ({
          ...item,
          resources: withParsedResources(item).resources,
          category: item.category ?? categoryFromMetadata(item.metadata)
        }))
        if (newActivitiesParsed && newActivitiesParsed.length > 0) {
          // Filter activities for the current date
          const currentDateStr = dayjs(currentDate).format('YYYY-MM-DD')
          const filteredActivities = newActivitiesParsed.filter((activity) => {
            const activityDateStr = dayjs(activity.start_time).format('YYYY-MM-DD')
            return activityDateStr === currentDateStr
          })

          if (filteredActivities.length > 0) {
            // Update last checked time to the latest activity's start time
            const latestActivity = filteredActivities[filteredActivities.length - 1]
            lastCheckedTimeRef.current = latestActivity.start_time
            // 新活动前插并去重（顺序沿用后端返回的批次顺序）
            setActivities((prev) => {
              const existingIds = new Set(prev.map((a) => a.id))
              const uniqueNewActivities = filteredActivities.filter((a) => !existingIds.has(a.id))
              return [...uniqueNewActivities, ...prev]
            })
          }
        }
      } catch (error) {
        logger.error('Failed to check new activity', { error })
      }
    }
    // Execute immediately
    checkNewActivities()
    // Set timer
    activityPollingRef.current = setInterval(checkNewActivities, 5000) // Poll every 5 seconds
  })

  // Stop polling for new activities
  const stopActivityPolling = useMemoizedFn(() => {
    if (activityPollingRef.current) {
      clearInterval(activityPollingRef.current)
      activityPollingRef.current = null
    }
  })

  // Start polling for recording stats
  const startStatsPolling = useMemoizedFn(() => {
    if (statsPollingRef.current) {
      clearInterval(statsPollingRef.current)
    }

    const fetchStats = async () => {
      try {
        if (!isToday || !isMonitoring) {
          return
        }
        const stats = await window.screenMonitorAPI.getRecordingStats()
        if (stats) {
          setRecordingStats(stats)
        }
      } catch (error) {
        logger.error('Failed to fetch recording stats', { error })
      }
    }

    // Execute immediately
    fetchStats()
    // Poll every 5 seconds
    statsPollingRef.current = setInterval(fetchStats, 5000)
  })

  // Stop polling for recording stats
  const stopStatsPolling = useMemoizedFn(() => {
    if (statsPollingRef.current) {
      clearInterval(statsPollingRef.current)
      statsPollingRef.current = null
    }
    setRecordingStats(null)
  })

  // Clean up polling on component unmount
  useEffect(() => {
    return () => {
      stopActivityPolling()
      stopStatsPolling()
    }
  }, [stopActivityPolling, stopStatsPolling])

  // Listen for lock/unlock screen events
  useObservableTask(
    {
      active: () => {
        isScreenLockedRef.current = true
        if (isMonitoring) {
          pauseMonitoring()
        }
      },
      inactive: () => {
        isScreenLockedRef.current = false
        if (isMonitoring) {
          resumeMonitoring()
        }
      }
    },
    'screen-monitor'
  )

  // Listen for tray toggle recording event (from Router.tsx when already on this page)
  useEffect(() => {
    const handleTrayToggleRecording = () => {
      if (isMonitoring) {
        stopMonitoring()
      } else {
        startMonitoring()
      }
    }

    window.addEventListener('tray-toggle-recording', handleTrayToggleRecording)

    return () => {
      window.removeEventListener('tray-toggle-recording', handleTrayToggleRecording)
    }
  }, [isMonitoring, startMonitoring, stopMonitoring])

  // Handle navigation state when coming from tray icon while on a different page
  useEffect(() => {
    const state = location.state as { toggleRecording?: boolean } | null
    if (state?.toggleRecording) {
      // Clear the navigation state first to prevent re-triggering
      navigate(location.pathname, { replace: true, state: {} })

      // Toggle recording based on current state
      if (isMonitoring) {
        stopMonitoring()
      } else {
        startMonitoring()
      }
    }
    // Only depend on location.state to avoid re-triggering when isMonitoring changes
    // startMonitoring and stopMonitoring are memoized so they're stable
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [location.state])

  const openSettings = useMemoizedFn(async () => {
    // Refresh the application list before opening settings
    try {
      setSettingsVisible(true)
      await Promise.all([refreshSourcesRead(), checkCanRecord()])
    } catch (error) {
      logger.error('Failed to refresh application list', { error })
    }
  })
  const [applicationVisible, setApplicationVisible] = useState(false)

  const handleCancelSettings = useMemoizedFn(() => {
    setTempRecordInterval(recordInterval)
    setTempEnableRecordingHours(enableRecordingHours)
    setTempRecordingHours(recordingHours as [string, string])
    setTempApplyToDays(applyToDays)
    setSettingsVisible(false)
    setApplicationVisible(false)
  })

  const handleSaveSettings = useMemoizedFn(() => {
    setRecordInterval(tempRecordInterval)
    setEnableRecordingHours(tempEnableRecordingHours)
    setRecordingHours(tempRecordingHours as [string, string])
    setApplyToDays(tempApplyToDays)
    setSettingsVisible(false)
  })

  // Check recording status on component mount
  useEffect(() => {
    checkCanRecord()
  }, [checkCanRecord])

  // Periodically check recording status
  useEffect(() => {
    let interval: NodeJS.Timeout | null = null
    if (isMonitoring && enableRecordingHours) {
      interval = setInterval(() => {
        checkCanRecord()
      }, 60000) // Check every minute
    }
    return () => {
      if (interval) {
        clearInterval(interval)
      }
    }
  }, [isMonitoring, enableRecordingHours, checkCanRecord])

  // Get sources
  const settingSources = useAppSelector((state) => state.captureSources.saved)
  const settingScreenSources = useMemo(
    () => (settingSources.state === 'hasData' ? get(settingSources, 'data.screenSources') : ([] as CaptureSource[])),
    [settingSources]
  )
  const settingWindowSources = useMemo(
    () => (settingSources.state === 'hasData' ? get(settingSources, 'data.appSources') : ([] as CaptureSource[])),
    [settingSources]
  )
  const [form] = Form.useForm<{ screenSources?: string[]; windowSources?: string[] }>()
  const entry = useMemoizedFn(async () => {
    const screenIds = settingScreenSources?.map((v) => v.id) || []
    const windowIds = settingWindowSources?.map((v) => v.id) || []
    const screenList = screenAllSources?.filter((source) => screenIds?.includes(source.id)) || []
    const windowList = appAllSources?.filter((source) => windowIds?.includes(source.id)) || []
    const screenSources = screenList.map((source) => source.id)
    form.setFieldsValue({
      screenSources: screenSources.length > 0 ? screenSources : [get(screenAllSources[0], 'id')].filter(Boolean),
      windowSources: windowList.map((source) => source.id)
    })
    await window.screenMonitorAPI.updateCurrentRecordApp([
      ...(screenList.length > 0 ? screenList : [get(screenAllSources, 0)].filter(Boolean)),
      ...windowList
    ])
  })

  // Tips: The biggest problem with using Form for management is that when the user does not select any screen or window, it will cause the save to fail
  const handleSave = useMemoizedFn(async () => {
    const values = form.getFieldsValue()
    if (![...(values.screenSources || []), ...(values.windowSources || [])].length) {
      Message.info(t('screenMonitor.selectSourceRequired'))
      return
    }
    const screenList = screenAllSources?.filter((source) => values.screenSources?.includes(source.id)) || []
    const windowList = appAllSources?.filter((source) => values.windowSources?.includes(source.id)) || []
    await window.screenMonitorAPI.setSettings('settings', {
      screenList,
      windowList
    })
    await window.screenMonitorAPI.updateCurrentRecordApp([...screenList, ...windowList])
    handleSaveSettings()
    await dispatch(refreshCaptureSourcesFromSettings())
  })

  useEffect(() => {
    if (settingSources.state === 'hasData' && sources.state === 'hasData') {
      entry()
      setTempRecordInterval(recordInterval)
      setTempEnableRecordingHours(enableRecordingHours)
      setTempRecordingHours(recordingHours as [string, string])
      setTempApplyToDays(applyToDays)
    }
  }, [settingSources, sources, entry, recordInterval, enableRecordingHours, recordingHours, applyToDays])

  const handleRequestPermission = useMemoizedFn(async () => {
    await grantPermission()
  })

  return (
    <div className="top-0 left-0 flex flex-col h-screen overflow-y-hidden pr-2 pb-2 pl-0 rounded-[20px] relative">
      <div style={{ height: '8px', appRegion: 'drag' } as React.CSSProperties} />
      <div className="bg-[var(--color-bg-2)] rounded-[16px] p-6 h-[calc(100%-8px)] flex flex-col overflow-y-auto overflow-x-hidden scrollbar-hide pb-2">
        <ScreenMonitorHeader
          hasPermission={hasPermission}
          isMonitoring={isMonitoring}
          isToday={isToday}
          screenAllSources={screenAllSources}
          appAllSources={appAllSources}
          onOpenSettings={openSettings}
          onStartMonitoring={startMonitoring}
          onStopMonitoring={stopMonitoring}
          onRequestPermission={handleRequestPermission}
        />

        {/* Recording area */}
        {(sources.state === 'hasError' || settingSources.state === 'hasError') && (
          <Alert
            type="error"
            content={t('screenMonitor.sourcesLoadFailed')}
            action={
              <Button
                onClick={() => {
                  void dispatch(refreshCaptureSources())
                  void dispatch(refreshCaptureSourcesFromSettings())
                }}>
                {t('common.retry')}
              </Button>
            }
          />
        )}
        <div className="w-full mb-0 mx-auto flex-1 flex flex-col">
          <div className="screen-monitor-capture-surface border border-dashed border-[var(--color-border-3)] rounded-[12px] p-[30px] bg-white transition-all duration-300 flex-1 flex flex-col overflow-auto">
            <DateNavigation
              hasPermission={hasPermission}
              currentDate={currentDate}
              isToday={isToday}
              onPreviousDay={handlePreviousDay}
              onNextDay={handleNextDay}
              onDateChange={handleDateChange}
              onSetCurrentDate={setCurrentDate}
              disabledDate={disabledDate}
            />
            {(isMonitoring && isToday) || activities.length > 0 || Object.keys(screenshots).length > 0 ? (
              <RecordingTimeline
                isMonitoring={isMonitoring}
                isToday={isToday}
                canRecord={canRecord}
                recordReason={recordBlockReason}
                activities={activities}
                recordingStats={recordingStats}
                // 拖选后把范围交给总结页：用 hash 而不是路由 hook，
                // 与仓库既有的 isOnHomePage 读 hash 保持一致，也不引入新依赖
                onSummarizeRange={(from, to) => {
                  window.location.hash = `#/summaries?from=${encodeURIComponent(from)}&to=${encodeURIComponent(to)}`
                }}
              />
            ) : (
              <EmptyStatePlaceholder
                hasPermission={hasPermission}
                isToday={isToday}
                onGrantPermission={grantPermission}
              />
            )}
          </div>
        </div>

        <Modal
          style={{ width: '60%', minHeight: '30%' }}
          title={t('screenMonitor.displayScreenshot')}
          visible={!!selectedImage}
          onCancel={() => setSelectedImage(null)}
          footer={null}>
          {selectedImage && (
            <Image src={selectedImage} alt="Display Screenshot" style={{ width: '100%', borderRadius: 8 }} />
          )}
        </Modal>

        <SettingsModal
          visible={settingsVisible}
          form={form}
          sources={sources}
          screenAllSources={screenAllSources}
          appAllSources={appAllSources}
          applicationVisible={applicationVisible}
          tempRecordInterval={tempRecordInterval}
          tempEnableRecordingHours={tempEnableRecordingHours}
          tempRecordingHours={tempRecordingHours}
          tempApplyToDays={tempApplyToDays}
          isMonitoring={isMonitoring}
          captureEnabled={captureEnabled}
          screenRecordingTcc={screenRecordingTcc}
          windowsReason={windowsReason}
          onCancel={handleCancelSettings}
          onSave={handleSave}
          onSetApplicationVisible={setApplicationVisible}
          onSetTempRecordInterval={setTempRecordInterval}
          onSetTempEnableRecordingHours={setTempEnableRecordingHours}
          onSetTempRecordingHours={setTempRecordingHours}
          onSetTempApplyToDays={setTempApplyToDays}
          onRequestPermission={handleRequestPermission}
        />
      </div>
    </div>
  )
}

export default ScreenMonitor
