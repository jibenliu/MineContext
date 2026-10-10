// 页面级测试：助手气泡的流式渲染。
//
// 断言用户能看到的三件事：① 逐块出字（同一条消息里长出来）；② 生成中与结束
// 是两种状态；③ 失败要显示出来（而不是一直转圈）。

import { configureHttpClient } from '@renderer/services/axios-config'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { AssistantStream } from './assistant-stream'

vi.mock('@renderer/hooks/use-vault', () => ({
  useVaults: () => ({
    selectedVaultId: null,
    setSelectedVaultId: () => undefined,
    getVaultRoots: () => []
  })
}))

const originalFetch = globalThis.fetch

beforeEach(() => {
  // 助手流要走真实契约：地址与 token 由适配层按 daemon 的 `runtime.json` 注入。
  // 没注入时服务会明确拒绝发请求（见 services/chat-stream-no-backend.page.test.ts）。
  configureHttpClient(45002, 'token-from-runtime-json')
})

afterEach(() => {
  globalThis.fetch = originalFetch
})

/** 用合成流替代网络：分片边界故意切在一帧中间（真实网络就是这样）。 */
function streamChunks(chunks: string[]): void {
  const encoder = new TextEncoder()
  let index = 0
  globalThis.fetch = (async () => ({
    ok: true,
    body: {
      getReader: () => ({
        read: async () =>
          index >= chunks.length
            ? { done: true, value: undefined }
            : { done: false, value: encoder.encode(chunks[index++]) }
      })
    }
  })) as unknown as typeof fetch
}

describe('助手流式气泡（5.41）', () => {
  it('逐块出字，并在结束时切到 completed', async () => {
    streamChunks([
      'data: {"type":"stream_chunk","content":"上午"}\n\n',
      'data: {"type":"stream_chunk","cont',
      'ent":"在写导入脚本"}\n\n',
      'data: {"type":"completed"}\n\n'
    ])

    render(<AssistantStream />)
    fireEvent.click(screen.getByText('问一句'))

    await waitFor(() => expect(screen.getByTestId('assistant-text')).toHaveTextContent('上午在写导入脚本'))
    // 完成之后状态区不该再显示任何内部状态名（idle/streaming/completed 是内部词）
    expect(screen.getByTestId('assistant-state')).toHaveTextContent(/^$/)
  })

  it('失败要显示出来，而不是一直转圈', async () => {
    streamChunks(['data: {"type":"fail","message":"模型不可用（已降级）"}\n\n'])

    render(<AssistantStream />)
    fireEvent.click(screen.getByText('问一句'))

    await waitFor(() => expect(screen.getByTestId('assistant-state')).toHaveTextContent('生成失败'))
    expect(screen.getByTestId('assistant-text')).toHaveTextContent('模型不可用')
  })
})

describe('助手对话历史（5.41 完整会话的第一步）', () => {
  it('答完一轮后历史里留下这一问一答', async () => {
    streamChunks(['data: {"type":"stream_chunk","content":"上午在写导入脚本"}\n\n', 'data: {"type":"completed"}\n\n'])

    render(<AssistantStream />)
    fireEvent.click(screen.getByText('问一句'))

    await waitFor(() => expect(screen.getByTestId('assistant-history')).toBeInTheDocument())
    expect(screen.getByTestId('assistant-history')).toHaveTextContent('我今天做了什么？')
    expect(screen.getByTestId('assistant-history')).toHaveTextContent('上午在写导入脚本')
  })

  it('「新会话」清空历史与当前回答', async () => {
    streamChunks(['data: {"type":"completed"}\n\n'])

    render(<AssistantStream />)
    fireEvent.click(screen.getByText('问一句'))
    await waitFor(() => expect(screen.getByTestId('assistant-history')).toBeInTheDocument())

    fireEvent.click(screen.getByText('新会话'))

    // 没有历史的对话界面，用户没法对照上一句回答 —— 清空必须是显式动作
    expect(screen.queryByTestId('assistant-history')).toBeNull()
    expect(screen.getByTestId('assistant-text')).toHaveTextContent('')
  })
})

describe('助手回答按 markdown 渲染（5.41）', () => {
  it('markdown 生效（加粗 / 列表）', async () => {
    streamChunks([
      'data: {"type":"stream_chunk","content":"**上午**在写脚本\\n\\n- 第一条"}\n\n',
      'data: {"type":"completed"}\n\n'
    ])

    render(<AssistantStream />)
    fireEvent.click(screen.getByText('问一句'))

    await waitFor(() => expect(screen.getByTestId('assistant-text').querySelector('strong')).not.toBeNull())
    const answer = screen.getByTestId('assistant-text')
    expect(answer.querySelector('li')).not.toBeNull()
  })

  it('回答里的原始 HTML **不**被渲染（模型输出不可信）', async () => {
    streamChunks([
      'data: {"type":"stream_chunk","content":"<img src=x onerror=alert(1)>你好"}\n\n',
      'data: {"type":"completed"}\n\n'
    ])

    render(<AssistantStream />)
    fireEvent.click(screen.getByText('问一句'))

    await waitFor(() => expect(screen.getByTestId('assistant-text').textContent).toContain('你好'))
    const answer = screen.getByTestId('assistant-text')
    // 关键：不能被当成 HTML 插入（否则就是注入面）
    expect(answer.querySelector('img')).toBeNull()
    expect(answer.innerHTML).toContain('&lt;img')
  })
})

describe('多会话（5.41：列表与切换）', () => {
  it('列出会话，切换时清掉当前气泡与历史', async () => {
    streamChunks(['data: {"type":"completed"}\n\n'])
    // 会话列表走 chatApi（渠道 v1:conversations）
    ;(window as unknown as Record<string, unknown>).chatApi = {
      listConversations: async () => [
        { id: 1, title: '上午的整理' },
        { id: 2, title: '下午的排查' }
      ]
    }

    render(<AssistantStream />)

    const select = await screen.findByLabelText('会话')
    expect(select).toHaveTextContent('上午的整理')
    expect(select).toHaveTextContent('下午的排查')

    // 先问一轮，让当前会话有历史
    fireEvent.click(screen.getByText('问一句'))
    await waitFor(() => expect(screen.getByTestId('assistant-history')).toBeInTheDocument())

    // 切到另一个会话：本地历史属于上一个会话，必须清掉
    fireEvent.change(select, { target: { value: '2' } })
    expect(screen.queryByTestId('assistant-history')).toBeNull()
    // 切会话后不应把内部状态名裸露给用户（例如把 `idle` 直接显示出来）
    expect(screen.getByTestId('assistant-state')).toHaveTextContent(/^$/)
  })

  it('切换后加载该会话的存档消息（否则旧对话看起来像丢了）', async () => {
    streamChunks(['data: {"type":"completed"}\n\n'])
    ;(window as unknown as Record<string, unknown>).chatApi = {
      listConversations: async () => [{ id: 7, title: '旧会话' }],
      listMessages: async (conversationId: number) => {
        expect(conversationId).toBe(7)
        return [
          { id: 1, role: 'user', content: '上周做了什么？' },
          { id: 2, role: 'assistant', content: '在改导入脚本' }
        ]
      }
    }

    render(<AssistantStream />)

    fireEvent.change(await screen.findByLabelText('会话'), { target: { value: '7' } })

    await waitFor(() => expect(screen.getByTestId('assistant-history')).toHaveTextContent('上周做了什么？'))
    expect(screen.getByTestId('assistant-history')).toHaveTextContent('在改导入脚本')
  })
})
