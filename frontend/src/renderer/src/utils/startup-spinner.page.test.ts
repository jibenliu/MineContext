// 启动占位（index.html 的 #spinner）必须被移除：它不会自己消失，
// React 挂载后如果还留在 DOM 里，就会一直盖在窗口中央。
//
// 调用时机：必须在 PersistGate rehydrate 之后（见 AppContent），不能在
// createRoot 后立刻拆 —— 否则 Tauri 窗口会出现「占位没了、页面还没画」的白屏。
import { describe, expect, it } from 'vitest'

import { removeStartupSpinner, STARTUP_SPINNER_ID } from './startup-spinner'

describe('startup-spinner', () => {
  it('移除 index.html 里的启动占位', () => {
    document.body.innerHTML = `<div id="root"></div><div id="${STARTUP_SPINNER_ID}"><img src="/logo.png" /></div>`

    expect(removeStartupSpinner()).toBe(true)
    expect(document.getElementById(STARTUP_SPINNER_ID)).toBeNull()
    // 只删占位，不动应用挂载点
    expect(document.getElementById('root')).not.toBeNull()
  })

  it('没有占位时是安全的空操作（幂等）', () => {
    document.body.innerHTML = '<div id="root"></div>'

    expect(removeStartupSpinner()).toBe(false)
    expect(removeStartupSpinner()).toBe(false)
    expect(document.getElementById('root')).not.toBeNull()
  })
})
