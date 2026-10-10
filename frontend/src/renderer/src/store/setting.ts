// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import { createSlice, PayloadAction } from '@reduxjs/toolkit'

export type ApplyToDays = 'weekday' | 'everyday'

export const defaultScreenSettings = {
  recordInterval: 15,
  enableRecordingHours: false,
  recordingHours: ['08:00:00', '20:00:00'] as [string, string],
  applyToDays: 'weekday' as ApplyToDays,
  /** 锁屏时暂停采集；默认开启，可在设置里关掉 */
  pauseOnLock: true
}

export type ScreenSettings = typeof defaultScreenSettings

const initialState = {
  systemNotificationsEnabled: true,
  screenSettings: defaultScreenSettings,
  // 界面语言：随 store 持久化，设置页有切换入口（见 i18n/index.ts）
  locale: 'zh-CN' as 'zh-CN' | 'en-US'
}

const settingSlice = createSlice({
  name: 'settings',
  initialState,
  reducers: {
    setSystemNotificationsEnabled(state, action: PayloadAction<boolean>) {
      state.systemNotificationsEnabled = action.payload
    },
    setScreenSettings(state, action: PayloadAction<Partial<ScreenSettings>>) {
      state.screenSettings = { ...state.screenSettings, ...action.payload }
    },
    setLocale(state, action: PayloadAction<'zh-CN' | 'en-US'>) {
      state.locale = action.payload
    }
  }
})

export const { setScreenSettings, setLocale, setSystemNotificationsEnabled } = settingSlice.actions

export default settingSlice.reducer
