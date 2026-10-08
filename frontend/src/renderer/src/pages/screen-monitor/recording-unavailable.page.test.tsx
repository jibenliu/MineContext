// 页面级测试：不能录制时必须说明原因，而不是笼统地说「不在录制时段」。
//
// 两者是不同的处置：「不能录制」要用户去处理（授权/显示器），「不在录制时段」等一会儿
// 就好。此前两种情况共用同一句文案，用户会被引到错误的方向。

import { installFakeBackend } from '@renderer/test/page-setup'
import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { RecordingTimeline } from './components/recording-timeline'

describe('时间线：不能录制时说明原因', () => {
  it('显示后端给的原因', () => {
    installFakeBackend({}, { strict: false })
    render(
      <RecordingTimeline
        isMonitoring
        isToday
        canRecord={false}
        activities={[]}
        recordingStats={null}
        recordReason="缺少屏幕录制权限：请到系统设置里授权"
      />
    )

    expect(screen.getByTestId('recording-unavailable-reason')).toHaveTextContent('缺少屏幕录制权限')
    // 不能录制时不该再说「不在录制时段」——那句会把用户引到错误的处置方向
    expect(screen.queryByText(/not in recording hours/i)).toBeNull()
  })

  it('后端没给原因时给一句兜底说明，而不是空白', () => {
    installFakeBackend({}, { strict: false })
    render(<RecordingTimeline isMonitoring isToday canRecord={false} activities={[]} recordingStats={null} />)

    // 文案已接入 i18n（默认中文，可切英文），所以这里两种语言都认：
    // 钉的是「有兜底说明」这件事，不是某个语言的具体字面
    expect(screen.getByTestId('recording-unavailable-reason')).toHaveTextContent(/not available|无法录制/i)
  })

  it('未在录制且可录制时，保持原有的「可以再次开始」提示', () => {
    installFakeBackend({}, { strict: false })
    render(<RecordingTimeline isMonitoring={false} isToday canRecord activities={[]} recordingStats={null} />)

    expect(screen.getByText(/You can start recording again|你可以再次开始录制/i)).toBeInTheDocument()
    expect(screen.queryByTestId('recording-unavailable-reason')).toBeNull()
  })
})
