// 助手引用可点：活动跳截图时间线，笔记跳 vault。

import store from '@renderer/store'
import { chatStreamService, StreamEvent } from '@renderer/services/chat-stream-service'
import { installFakeBackend } from '@renderer/test/page-setup'
import { act, fireEvent, render, screen, within } from '@testing-library/react'
import { beforeEach, expect, it, vi } from 'vitest'
import { Provider } from 'react-redux'
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom'

import { AssistantStream } from './assistant-stream'

type Emit = (event: StreamEvent) => void
let emit: Emit

beforeEach(() => {
  installFakeBackend({}, { strict: false })
  vi.spyOn(chatStreamService, 'sendStreamMessage').mockImplementation(async (_request, onEvent) => {
    emit = onEvent
  })
})

function LocationProbe() {
  const location = useLocation()
  return <div data-testid="location">{`${location.pathname}${location.search}`</div>
}

function renderAssistant() {
  return render(
    <Provider store={store}>
      <MemoryRouter initialEntries={['/assistant']}>
        <Routes>
          <Route
            path="*"
            element={
              <>
                <LocationProbe />
                <AssistantStream />
              </>
            }
          />
        </Routes>
      </MemoryRouter>
    </Provider>
  )
}

it('点击活动引用跳到屏幕监控并带上 activity / at', () => {
  renderAssistant()
  fireEvent.click(screen.getByText('问一句'))
  act(() => {
    emit({
      type: 'stream_complete',
      content: '依据如下',
      citations: [
        { document_id: 'act-7', title: '导入工作', kind: 'activity', at: 1_725_000_000_000 }
      ]
    } as StreamEvent)
  })
  const history = screen.getByTestId('assistant-history')
  const citation = within(history).getByTestId('assistant-citation')
  expect(citation).toHaveTextContent('导入工作')
  fireEvent.click(citation)
  expect(screen.getByTestId('location')).toHaveTextContent(
    '/screen-monitor?activity=act-7&at=1725000000000'
  )
})

it('点击笔记引用跳到 vault', () => {
  renderAssistant()
  fireEvent.click(screen.getByText('问一句'))
  act(() => {
    emit({
      type: 'stream_complete',
      content: '见笔记',
      citations: [{ document_id: 'note-42', title: '季度复盘', kind: 'document' }]
    } as StreamEvent)
  })
  fireEvent.click(within(screen.getByTestId('assistant-history')).getByTestId('assistant-citation'))
  expect(screen.getByTestId('location')).toHaveTextContent('/vault?id=42')
})
