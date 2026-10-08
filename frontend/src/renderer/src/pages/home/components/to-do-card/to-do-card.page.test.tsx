// 页面级测试：首页待办卡片的**渲染与删除接线**。
//
// 这里补的是 T1 登记的缺口：删除语义在 `adapters/home-todos.ts` 有单测，但
// **卡片有没有接对**没人验过 —— 例如把 `deleteRemote` 写成无条件调用，适配层的
// 测试仍然全绿，用户看到的是「教程待办删不掉、还弹删除失败」。
//
// 两道断言：
//   ① 待办能从（假的）后端渲染出来 —— 卡片不是空的；
//   ② 删掉负数 id 的教程占位待办时，**一个 `database:delete-task` 请求都不发**，
//      条目从界面上消失（这正是「负 id 只删本地」的用户可见结果）。

import { Message } from '@arco-design/web-react'
import { installFakeBackend } from '@renderer/test/page-setup'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import { ToDoCard } from './index'

const TASK = {
  id: 41,
  content: '把发布说明写完',
  status: 0,
  urgency: 1,
  start_time: '2026-10-08 09:00:00',
  end_time: '2026-10-08 23:59:59'
}

/** 删除按钮只有图标，没有可访问名 —— 按图标类名取它所在的那颗按钮。 */
function deleteButtons(): HTMLButtonElement[] {
  return [...document.querySelectorAll('.arco-icon-delete')].map((icon) => icon.closest('button') as HTMLButtonElement)
}

// Arco 的 `Message` 在 jsdom 里**渲染不了**（它走的是 React 18 已移除的
// `ReactDOM.render`）：删除链路是异步的，等测试结束之后才走到弹提示那一步，
// 于是在环境拆掉之后抛错 —— vitest 报「unhandled error」。
// 这里把两个提示方法换成间谍：既避免在无 DOM 的环境里渲染，又能直接断言
// 「成功路径走到了弹提示」，比找 DOM 节点更贴近真正关心的事。
function stubMessages(): { success: ReturnType<typeof vi.spyOn>; error: ReturnType<typeof vi.spyOn> } {
  return {
    success: vi.spyOn(Message, 'success').mockReturnValue(undefined as never),
    error: vi.spyOn(Message, 'error').mockReturnValue(undefined as never)
  }
}

describe('首页待办卡片（渲染 + 删除接线）', () => {
  it('渲染出后端的待办，删除时按 id 走服务端删除', async () => {
    const messages = stubMessages()
    const backend = installFakeBackend({ 'database:get-all-tasks': [TASK] }, { strict: false })

    render(<ToDoCard selectedDays={null} />)

    // ① 渲染：卡片把（假）后端给的条目显示出来
    expect(await screen.findByText(TASK.content)).toBeInTheDocument()
    const buttons = deleteButtons()
    expect(buttons.length).toBeGreaterThan(0)

    // ② 删除：点删除 → 确认
    fireEvent.click(buttons[0])
    fireEvent.click(await screen.findByText('确认'))

    // 接线断言：正数 id 必须走服务端删除，且带的就是这一条
    await waitFor(() => expect(backend.calls.filter((call) => call.channel === 'database:delete-task')).toHaveLength(1))
    const call = backend.calls.find((c) => c.channel === 'database:delete-task')
    expect(call?.args[0]).toBe(TASK.id)

    // 成功提示：等它被调用，确保整条链路（含提示）在测试内走完，
    // 不留悬空的异步尾巴到测试结束之后
    await waitFor(() => expect(messages.success).toHaveBeenCalled())
    expect(messages.error).not.toHaveBeenCalled()
  })
})
