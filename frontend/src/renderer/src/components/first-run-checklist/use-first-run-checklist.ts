// 采集首次引导清单所需的现场状态，并在全部完成时写入持久化完成标记。

import {
  currentFirstRunStep,
  deriveFirstRunSteps,
  type FirstRunSnapshot,
  isFirstRunAllDone,
  shouldShowFirstRunChecklist
} from '@renderer/adapters/first-run-checklist'
import { isScreenRecordingGranted } from '@renderer/hooks/use-screen'
import { getModelInfo } from '@renderer/services/settings'
import { useAppDispatch, useAppSelector } from '@renderer/store'
import { setFirstRunOnboardingComplete } from '@renderer/store/setting'
import { getLogger } from '@shared/logger/renderer'
import { useMemoizedFn } from 'ahooks'
import { useEffect, useState } from 'react'

const logger = getLogger('FirstRunChecklist')

const POLL_MS = 4000

export function useFirstRunChecklist(opts?: { apiKeyConfiguredOverride?: boolean }) {
  const dispatch = useAppDispatch()
  const completedPersisted = useAppSelector((s) =>
    Boolean((s.setting as { firstRunOnboardingComplete?: boolean }).firstRunOnboardingComplete)
  )
  const [permissionGranted, setPermissionGranted] = useState(false)
  const [apiKeyConfigured, setApiKeyConfigured] = useState(false)
  const [isRecording, setIsRecording] = useState(false)
  const [hasScreenshot, setHasScreenshot] = useState(false)
  const [waitingCleared, setWaitingCleared] = useState(false)

  const refresh = useMemoizedFn(async () => {
    try {
      if (typeof window.screenMonitorAPI?.checkPermissions === 'function') {
        const result = await window.screenMonitorAPI.checkPermissions()
        setPermissionGranted(isScreenRecordingGranted(result))
      }
    } catch (error) {
      logger.warn('[first-run] 读屏幕录制权限失败', error)
    }

    if (opts?.apiKeyConfiguredOverride !== undefined) {
      setApiKeyConfigured(opts.apiKeyConfiguredOverride)
    } else {
      try {
        const info = await getModelInfo()
        setApiKeyConfigured(Boolean(info?.hasApiKey))
      } catch (error) {
        logger.warn('[first-run] 读模型配置失败', error)
      }
    }

    try {
      if (typeof window.screenMonitorAPI?.getRecordingStats === 'function') {
        const stats = (await window.screenMonitorAPI.getRecordingStats()) as {
          captured_screenshots?: number
        }
        setHasScreenshot(Number(stats?.captured_screenshots ?? 0) > 0)
      }
    } catch (error) {
      logger.warn('[first-run] 读录制统计失败', error)
    }
  })

  useEffect(() => {
    void refresh()
    const timer = window.setInterval(() => void refresh(), POLL_MS)
    return () => window.clearInterval(timer)
  }, [refresh, opts?.apiKeyConfiguredOverride])

  useEffect(() => {
    const push = window.serverPushAPI?.pushScreenMonitorStatus
    if (typeof push !== 'function') return
    return push((status) => {
      setIsRecording(status === 'running')
    })
  }, [])

  const snapshot: FirstRunSnapshot = {
    permissionGranted,
    apiKeyConfigured: opts?.apiKeyConfiguredOverride !== undefined ? opts.apiKeyConfiguredOverride : apiKeyConfigured,
    isRecording,
    hasScreenshot,
    waitingCleared,
    completedPersisted
  }

  const visible = shouldShowFirstRunChecklist(snapshot)
  const steps = deriveFirstRunSteps(snapshot)
  const current = currentFirstRunStep(snapshot)
  const allDone = isFirstRunAllDone(snapshot)

  useEffect(() => {
    if (!completedPersisted && allDone) {
      dispatch(setFirstRunOnboardingComplete(true))
    }
  }, [allDone, completedPersisted, dispatch])

  const dismiss = useMemoizedFn(() => {
    dispatch(setFirstRunOnboardingComplete(true))
  })

  const clearWaiting = useMemoizedFn(() => {
    setWaitingCleared(true)
  })

  const requestPermission = useMemoizedFn(async () => {
    try {
      const result = await window.screenMonitorAPI?.openPrefs?.()
      if (isScreenRecordingGranted(result)) {
        setPermissionGranted(true)
        return
      }
    } catch (error) {
      logger.warn('[first-run] 请求屏幕录制权限失败', error)
    }
    window.setTimeout(() => void refresh(), 3000)
  })

  return {
    visible,
    steps,
    current,
    snapshot,
    dismiss,
    clearWaiting,
    requestPermission,
    refresh
  }
}
