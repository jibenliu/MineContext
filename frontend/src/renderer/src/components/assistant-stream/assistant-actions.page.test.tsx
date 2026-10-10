import { chatStreamService, StreamEvent } from '@renderer/services/chat-stream-service'
import { messageService } from '@renderer/services/messages-service'
import store from '@renderer/store'
import { installFakeBackend } from '@renderer/test/page-setup'
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { Provider } from 'react-redux'
import { MemoryRouter } from 'react-router-dom'

import { AssistantStream } from './assistant-stream'

vi.mock('@renderer/hooks/use-vault', () => ({
  useVaults: () => ({
    selectedVaultId: null,
    setSelectedVaultId: () => undefined,
    getVaultRoots: () => []
  })
}))

type Emit = (event: StreamEvent) => void
let emit: Emit
let finish: (() => void) | undefined

afterEach(() => vi.unstubAllGlobals())

beforeEach(() => {
  installFakeBackend({}, { strict: false })
  vi.spyOn(chatStreamService, 'sendStreamMessage').mockImplementation(
    async (_request, onEvent, _onError, onComplete) => {
      emit = onEvent
      finish = onComplete
    }
  )
})

function event(payload: Record<string, unknown>) {
  emit(payload as unknown as StreamEvent)
}

function renderStream() {
  return render(
    <Provider store={store}>
      <MemoryRouter>
        <AssistantStream />
      </MemoryRouter>
    </Provider>
  )
}

it('回答可复制、重新回答，并展示真实处理进度与引用', async () => {
  const writeText = vi.fn().mockResolvedValue(undefined)
  vi.stubGlobal('navigator', { clipboard: { writeText } })
  renderStream()
  fireEvent.click(screen.getByText('问一句'))
  act(() => {
    event({ type: 'thinking', content: '正在检索本地记录', stage: 'context_gathering' })
  })
  expect(screen.getByText('正在检索本地记录')).toBeInTheDocument()
  act(() => {
    event({
      type: 'stream_complete',
      content: '**真实回答**',
      citations: [{ document_id: 'act-7', title: '导入工作', kind: 'activity' }]
    })
    event({ type: 'done' })
    finish?.()
  })
  const history = screen.getByTestId('assistant-history')
  expect(within(history).getByText(/导入工作/)).toBeInTheDocument()
  expect(history.querySelector('strong')).toHaveTextContent('真实回答')
  fireEvent.click(screen.getByRole('button', { name: '复制回答' }))
  await waitFor(() => expect(writeText).toHaveBeenCalledWith('**真实回答**'))
  fireEvent.click(screen.getByRole('button', { name: '重新回答' }))
  expect(chatStreamService.sendStreamMessage).toHaveBeenCalledTimes(2)
  vi.unstubAllGlobals()
})

it('生成中拒绝重复提交，停止会调用真实中断接口并忽略迟到帧', async () => {
  const interrupt = vi
    .spyOn(messageService, 'interruptMessageGeneration')
    .mockResolvedValue({ status: 'success', message_id: 99 } as never)
  const abort = vi.spyOn(chatStreamService, 'abortStream')
  renderStream()
  const input = screen.getByLabelText('输入问题，回车发送')
  fireEvent.change(input, { target: { value: '我的活动' } })
  fireEvent.keyDown(input, { key: 'Enter', keyCode: 13 })
  fireEvent.keyDown(input, { key: 'Enter', keyCode: 13 })
  expect(chatStreamService.sendStreamMessage).toHaveBeenCalledTimes(1)
  act(() => event({ type: 'session_start', conversation_id: 8, assistant_message_id: 99 }))
  fireEvent.click(screen.getByRole('button', { name: '停止生成' }))
  await waitFor(() => expect(interrupt).toHaveBeenCalledWith(99))
  await waitFor(() => expect(abort).toHaveBeenCalled())
  act(() => {
    event({ type: 'stream_chunk', content: '迟到回答' })
    finish?.()
  })
  expect(screen.queryByText('迟到回答')).toBeNull()
  expect(screen.getByTestId('assistant-state')).toHaveTextContent('已停止')
  fireEvent.click(screen.getByText('新会话'))
  fireEvent.click(screen.getByText('问一句'))
  expect(vi.mocked(chatStreamService.sendStreamMessage).mock.calls[1][0].conversation_id).toBeUndefined()
})

it('失败只记录一轮，重新回答可发起新请求', () => {
  renderStream()
  fireEvent.click(screen.getByText('问一句'))
  act(() => {
    event({ type: 'fail', message: '模型暂不可用' })
    finish?.()
  })
  expect(within(screen.getByTestId('assistant-history')).getAllByRole('listitem')).toHaveLength(1)
  fireEvent.click(screen.getByRole('button', { name: '重新回答' }))
  expect(chatStreamService.sendStreamMessage).toHaveBeenCalledTimes(2)
})

it('切换会话忽略迟到历史，并恢复已保存的来源', async () => {
  let resolveFirst!: (rows: unknown[]) => void
  ;(window as unknown as Record<string, unknown>).chatApi = {
    listConversations: async () => [
      { id: 1, title: '会话一' },
      { id: 2, title: '会话二' }
    ],
    listMessages: async (id: number) =>
      id === 1
        ? new Promise((resolve) => {
            resolveFirst = resolve
          })
        : [
            { id: 3, role: 'user', content: '第二个问题' },
            {
              id: 4,
              role: 'assistant',
              content: '第二个回答',
              metadata: JSON.stringify({ sources: [{ document_id: 'note-2', title: '持久化引用', kind: 'note' }] })
            }
          ]
  }
  renderStream()
  const select = await screen.findByLabelText('会话')
  fireEvent.change(select, { target: { value: '1' } })
  fireEvent.change(select, { target: { value: '2' } })
  await waitFor(() => expect(screen.getByTestId('assistant-history')).toHaveTextContent('持久化引用'))
  await act(async () => {
    resolveFirst([
      { role: 'user', content: '过时问题' },
      { role: 'assistant', content: '过时回答' }
    ])
  })
  expect(screen.queryByText('过时回答')).toBeNull()
  expect(screen.getByTestId('assistant-text')).toHaveTextContent('第二个回答')
})

it('复制失败显示反馈，不产生未处理拒绝', async () => {
  vi.stubGlobal('navigator', { clipboard: { writeText: vi.fn().mockRejectedValue(new Error('denied')) } })
  renderStream()
  fireEvent.click(screen.getByText('问一句'))
  act(() => {
    event({ type: 'stream_complete', content: '可以手动复制的回答' })
    finish?.()
  })
  fireEvent.click(screen.getByRole('button', { name: '复制回答' }))
  expect(await screen.findByRole('status')).toHaveTextContent('复制失败')
})

it('发送后立刻在历史里看到用户提问（不能等流结束，否则像输入消失）', async () => {
  renderStream()
  const input = screen.getByLabelText('输入问题，回车发送')
  fireEvent.change(input, { target: { value: '我的活动呢' } })
  fireEvent.keyDown(input, { key: 'Enter', keyCode: 13 })
  await waitFor(() => expect(screen.getByTestId('assistant-history')).toHaveTextContent('我的活动呢'))
  expect(input).toHaveValue('')
})

it('无标题会话显示未命名文案，不用裸数字 id', async () => {
  ;(window as unknown as Record<string, unknown>).chatApi = {
    listConversations: async () => [
      { id: 2, title: null },
      { id: 1, title: '   ' }
    ]
  }
  renderStream()
  expect(await screen.findAllByRole('button', { name: '未命名会话' })).toHaveLength(2)
  expect(screen.queryByRole('button', { name: '2' })).toBeNull()
  expect(screen.queryByRole('button', { name: '1' })).toBeNull()
})

it('删除会话会确认后调用持久化接口，并从列表移除；删当前会话回到新会话', async () => {
  const { Modal } = await import('@arco-design/web-react')
  vi.spyOn(Modal, 'confirm').mockImplementation(((config: { onOk?: () => void }) => {
    config.onOk?.()
    return { update: () => undefined, close: () => undefined }
  }) as typeof Modal.confirm)

  const deleted: number[] = []
  let listed = [
    { id: 1, title: '会话一' },
    { id: 2, title: '会话二' }
  ]
  ;(window as unknown as Record<string, unknown>).chatApi = {
    listConversations: async () => listed,
    listMessages: async (id: number) =>
      id === 1
        ? [
            { id: 10, role: 'user', content: '第一个问题' },
            { id: 11, role: 'assistant', content: '第一个回答' }
          ]
        : [],
    deleteConversation: async (id: number) => {
      deleted.push(id)
      listed = listed.filter((row) => row.id !== id)
      return { success: true, id }
    }
  }

  renderStream()
  expect(await screen.findByRole('button', { name: '会话一' })).toBeInTheDocument()
  fireEvent.click(screen.getByRole('button', { name: '会话一' }))
  await waitFor(() => expect(screen.getByTestId('assistant-history')).toHaveTextContent('第一个回答'))

  fireEvent.click(screen.getByRole('button', { name: '删除 会话一' }))
  await waitFor(() => expect(deleted).toEqual([1]))
  await waitFor(() => expect(screen.queryByRole('button', { name: '会话一' })).toBeNull())
  expect(screen.getByRole('button', { name: '会话二' })).toBeInTheDocument()
  expect(screen.queryByTestId('assistant-history')).toBeNull()
  expect(screen.getByLabelText('会话')).toHaveValue('')
})
