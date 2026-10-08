import type { CaptureSource } from '@interface/common/source'
import { combineReducers, createAsyncThunk, createSlice } from '@reduxjs/toolkit'
import { formatName } from '@renderer/utils/format-name'

interface SourceGroups {
  screenSources: CaptureSource[]
  appSources: CaptureSource[]
}

interface SourceState {
  state: 'idle' | 'loading' | 'hasData' | 'hasError'
  data: SourceGroups
  error: string | null
  requestId: string | null
}

const createSourceSlice = (name: string, fetchSources: () => Promise<SourceGroups>) => {
  const refresh = createAsyncThunk(`captureSources/${name}/refresh`, fetchSources)
  const initialState: SourceState = {
    state: 'idle',
    data: { screenSources: [], appSources: [] },
    error: null,
    requestId: null
  }
  const slice = createSlice({
    name: `captureSources/${name}`,
    initialState,
    reducers: {},
    extraReducers: (builder) => {
      builder.addCase(refresh.pending, (state, action) => {
        state.state = 'loading'
        state.error = null
        state.requestId = action.meta.requestId
      })
      builder.addCase(refresh.fulfilled, (state, action) => {
        if (state.requestId !== action.meta.requestId) return
        state.state = 'hasData'
        state.data = action.payload
        state.requestId = null
      })
      builder.addCase(refresh.rejected, (state, action) => {
        if (state.requestId !== action.meta.requestId) return
        state.state = action.meta.aborted ? 'idle' : 'hasError'
        state.error = action.meta.aborted ? null : action.error.message || 'Failed to load capture sources'
        state.requestId = null
      })
    }
  })
  return { refresh, reducer: slice.reducer }
}

const available = createSourceSlice('available', async () => {
  const result = await window.screenMonitorAPI.getCaptureAllSources()
  if (!result.success) throw new Error(result.error || 'Failed to load capture sources')
  const sources: CaptureSource[] = result.sources || []
  return {
    screenSources: formatName(sources.filter((source) => source.type === 'screen')),
    appSources: formatName(sources.filter((source) => source.type === 'window'))
  }
})

const saved = createSourceSlice('saved', async () => {
  const settings = await window.screenMonitorAPI.getSettings<{
    screenList?: CaptureSource[]
    windowList?: CaptureSource[]
  } | null>('settings')
  return {
    screenSources: formatName(settings?.screenList),
    appSources: formatName(settings?.windowList)
  }
})

export const refreshCaptureSources = available.refresh
export const refreshCaptureSourcesFromSettings = saved.refresh
export default combineReducers({ available: available.reducer, saved: saved.reducer })
