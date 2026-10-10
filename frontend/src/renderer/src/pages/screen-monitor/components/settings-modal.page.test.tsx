import { Form } from '@arco-design/web-react'
import { render, screen } from '@testing-library/react'
import type { ComponentProps } from 'react'
import { describe, expect, it, vi } from 'vitest'

import SettingsModal from './settings-modal'

function renderModal(
  props: Partial<ComponentProps<typeof SettingsModal>> & {
    isMonitoring: boolean
    captureEnabled?: boolean
    windowsReason?: string | null
    screenRecordingTcc?: boolean
  }
) {
  const Wrapper = () => {
    const [form] = Form.useForm()
    return (
      <SettingsModal
        visible
        form={form}
        sources={{ state: 'hasData' }}
        screenAllSources={[]}
        appAllSources={[]}
        applicationVisible
        tempRecordInterval={5}
        tempEnableRecordingHours={false}
        tempPauseOnLock
        tempRecordingHours={['09:00', '18:00']}
        tempApplyToDays="everyday"
        onCancel={vi.fn()}
        onSave={vi.fn()}
        onSetApplicationVisible={vi.fn()}
        onSetTempRecordInterval={vi.fn()}
        onSetTempEnableRecordingHours={vi.fn()}
        onSetTempPauseOnLock={vi.fn()}
        onSetTempRecordingHours={vi.fn()}
        onSetTempApplyToDays={vi.fn()}
        {...props}
      />
    )
  }
  return render(<Wrapper />)
}

describe('SettingsModal recording / window alerts', () => {
  it('enabled=false 且未录制时提示录制尚未开始', () => {
    renderModal({ isMonitoring: false, captureEnabled: false })
    expect(screen.getByTestId('recording-not-started-alert')).toBeInTheDocument()
  })

  it('正在录制时不提示尚未开始', () => {
    renderModal({ isMonitoring: true, captureEnabled: true })
    expect(screen.queryByTestId('recording-not-started-alert')).not.toBeInTheDocument()
  })

  it('windows_reason 为权限时显示窗口权限 Alert', () => {
    renderModal({
      isMonitoring: false,
      captureEnabled: true,
      windowsReason: 'screen_recording_permission',
      onRequestPermission: vi.fn()
    })
    expect(screen.getByTestId('window-permission-alert')).toBeInTheDocument()
    expect(screen.queryByText('只能选择当前已打开的应用程序')).not.toBeInTheDocument()
  })

  it('展示 TCC / 录制状态 / 窗口列表原因，并在 TCC 未授权时给出退出重开提示与 CTA', () => {
    renderModal({
      isMonitoring: false,
      captureEnabled: false,
      screenRecordingTcc: false,
      windowsReason: 'screen_recording_permission',
      onRequestPermission: vi.fn()
    })
    expect(screen.getByTestId('capture-status-panel')).toBeInTheDocument()
    expect(screen.getByTestId('capture-tcc-status')).toHaveTextContent(/未授权|Denied|not granted/i)
    expect(screen.getByTestId('capture-recording-status')).toHaveTextContent(/已停止|Stopped|not started/i)
    expect(screen.getByTestId('capture-window-list-status')).toHaveTextContent(/权限|permission/i)
    expect(screen.getByTestId('quit-relaunch-hint')).toBeInTheDocument()
    expect(screen.getAllByRole('button', { name: /打开系统设置|Open System Settings/i }).length).toBeGreaterThanOrEqual(
      1
    )
  })

  it('windows_reason=empty 时说明无打开窗口，而不是权限', () => {
    renderModal({
      isMonitoring: true,
      captureEnabled: true,
      screenRecordingTcc: true,
      windowsReason: 'empty'
    })
    expect(screen.getByTestId('capture-window-list-status')).toHaveTextContent(/没有打开|no open/i)
    expect(screen.queryByTestId('window-permission-alert')).not.toBeInTheDocument()
    expect(screen.queryByTestId('quit-relaunch-hint')).not.toBeInTheDocument()
  })

  it('默认展示锁屏暂停开关且为开启', () => {
    renderModal({ isMonitoring: true, captureEnabled: true })
    const toggle = screen.getByTestId('pause-on-lock-switch')
    expect(toggle).toBeInTheDocument()
    expect(toggle).toHaveAttribute('aria-checked', 'true')
  })
})
