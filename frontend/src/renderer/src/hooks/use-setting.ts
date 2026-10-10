// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import { RootState, useAppDispatch } from '@renderer/store'
import { ApplyToDays, setScreenSettings as setScreenSettingsAction } from '@renderer/store/setting'
import { useCallback } from 'react'
import { useSelector } from 'react-redux'

export const useSetting = () => {
  const dispatch = useAppDispatch()
  const screenSettings = useSelector((state: RootState) => state.setting.screenSettings)

  const { recordInterval, recordingHours, enableRecordingHours, applyToDays, pauseOnLock } =
    screenSettings

  const setRecordInterval = useCallback(
    (interval: number) => {
      dispatch(setScreenSettingsAction({ recordInterval: interval }))
    },
    [dispatch]
  )

  const setEnableRecordingHours = useCallback(
    (enable: boolean) => {
      dispatch(setScreenSettingsAction({ enableRecordingHours: enable }))
    },
    [dispatch]
  )

  const setRecordingHours = useCallback(
    (hours: [string, string]) => {
      dispatch(setScreenSettingsAction({ recordingHours: hours }))
    },
    [dispatch]
  )

  const setApplyToDays = useCallback(
    (days: ApplyToDays) => {
      dispatch(setScreenSettingsAction({ applyToDays: days }))
    },
    [dispatch]
  )

  const setPauseOnLock = useCallback(
    (enabled: boolean) => {
      dispatch(setScreenSettingsAction({ pauseOnLock: enabled }))
    },
    [dispatch]
  )

  return {
    recordInterval,
    recordingHours,
    enableRecordingHours,
    applyToDays,
    pauseOnLock,
    setRecordInterval,
    setEnableRecordingHours,
    setRecordingHours,
    setApplyToDays,
    setPauseOnLock
  }
}
