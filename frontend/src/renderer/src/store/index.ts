// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import { combineReducers, configureStore } from '@reduxjs/toolkit'
import { getLogger } from '@shared/logger/renderer'
import { useDispatch, useSelector, useStore } from 'react-redux'
import {
  createMigrate,
  FLUSH,
  PAUSE,
  PERSIST,
  persistReducer,
  persistStore,
  PURGE,
  REGISTER,
  REHYDRATE
} from 'redux-persist'
import storage from 'redux-persist/lib/storage'

import storeSyncService from '../services/store-sync-service'
import captureSources from './capture-sources'
import chatHistory from './chat-history'
import events from './events'
import { migrations } from './migrations'
import navigation from './navigation'
import screen from './screen'
import setting from './setting'
import vault from './vault'

const logger = getLogger('Store')

const rootReducer = combineReducers({
  captureSources,
  navigation,
  setting,
  screen,
  vault,
  events,
  chatHistory
})

// Desktop application persistence configuration, mainly for data recovery on next launch
const persistedReducer = persistReducer(
  {
    key: 'minecontext',
    storage,
    version: 2,
    migrate: createMigrate(migrations, { debug: false }),
    blacklist: ['vault', 'screen', 'chatHistory', 'captureSources'] // Do not persist vault, vault data is stored in the sqlite data table
  },
  rootReducer
)

const store = configureStore({
  // @ts-expect-error 持久化后的 reducer 类型与 rootReducer 不完全一致
  reducer: persistedReducer as typeof rootReducer,
  middleware: (getDefaultMiddleware) => {
    return getDefaultMiddleware({
      serializableCheck: {
        ignoredActions: [FLUSH, REHYDRATE, PAUSE, PERSIST, PURGE, REGISTER]
      }
    }).concat(storeSyncService.createMiddleware())
  },
  devTools: true
})
storeSyncService.init(store, {
  syncList: ['screen/', 'setting/']
})

export type RootState = ReturnType<typeof rootReducer>
export type AppDispatch = typeof store.dispatch

export const persistor = persistStore(store)
export const useAppDispatch = useDispatch.withTypes<AppDispatch>()
export const useAppSelector = useSelector.withTypes<RootState>()
export const useAppStore = useStore.withTypes<typeof store>()

export async function handleSaveData() {
  logger.info('Flushing redux persistor data')
  await persistor.flush()
  logger.info('Flushed redux persistor data')
}

export default store
