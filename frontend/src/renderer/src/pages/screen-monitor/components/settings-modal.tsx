import { Alert, Button, Checkbox, Form, Modal, Radio, Slider, Spin, Switch, TimePicker } from '@arco-design/web-react'
import screenIcon from '@renderer/assets/icons/screen.svg'
import { useI18n } from '@renderer/i18n'
import { ApplyToDays } from '@renderer/store/setting'
import clsx from 'clsx'
import React from 'react'

import {
  captureRecordingState,
  isWindowListPermissionBlocked,
  shouldShowQuitRelaunchHint,
  shouldShowRecordingNotStarted,
  shouldShowTccDenied,
  windowListHintKind
} from '../recording-status-copy'
import { Application } from './application'

interface SettingsModalProps {
  visible: boolean
  form: any
  sources: any
  screenAllSources: any[]
  appAllSources: any[]
  applicationVisible: boolean
  tempRecordInterval: number
  tempEnableRecordingHours: boolean
  tempRecordingHours: [string, string]
  tempApplyToDays: string
  /** 是否正在录制；false 时 interval 滑块不等于采集环在跑 */
  isMonitoring: boolean
  /** capture.enabled；false = 用户停过录 */
  captureEnabled?: boolean
  /** 进程 TCC 原值（与经验证后的 screen_recording 区分） */
  screenRecordingTcc?: boolean
  /** 后端 windows_reason */
  windowsReason?: string | null
  onCancel: () => void
  onSave: () => void
  onSetApplicationVisible: (visible: boolean) => void
  onSetTempRecordInterval: (value: number) => void
  onSetTempEnableRecordingHours: (value: boolean) => void
  onSetTempRecordingHours: (value: [string, string]) => void
  onSetTempApplyToDays: (value: ApplyToDays) => void
  onRequestPermission?: () => void
}

const SettingsModal: React.FC<SettingsModalProps> = ({
  visible,
  form,
  sources,
  screenAllSources,
  appAllSources,
  applicationVisible,
  tempRecordInterval,
  tempEnableRecordingHours,
  tempRecordingHours,
  tempApplyToDays,
  isMonitoring,
  captureEnabled,
  screenRecordingTcc,
  windowsReason,
  onCancel,
  onSave,
  onSetApplicationVisible,
  onSetTempRecordInterval,
  onSetTempEnableRecordingHours,
  onSetTempRecordingHours,
  onSetTempApplyToDays,
  onRequestPermission
}) => {
  const { t } = useI18n()
  const showNotStarted = shouldShowRecordingNotStarted(isMonitoring, captureEnabled)
  const showWindowPermission = isWindowListPermissionBlocked(windowsReason)
  const showTccDenied = shouldShowTccDenied(screenRecordingTcc)
  const showQuitRelaunch = shouldShowQuitRelaunchHint(screenRecordingTcc, windowsReason)
  const recordingState = captureRecordingState(isMonitoring, captureEnabled)
  const windowHint = windowListHintKind(windowsReason)

  const tccLabel =
    screenRecordingTcc === true
      ? t('screenMonitor.settings.tccGranted')
      : screenRecordingTcc === false
        ? t('screenMonitor.settings.tccDenied')
        : t('screenMonitor.settings.tccUnknown')

  const recordingLabel =
    recordingState === 'running'
      ? t('screenMonitor.settings.recordingRunning')
      : recordingState === 'enabled_idle'
        ? t('screenMonitor.settings.recordingEnabledIdle')
        : t('screenMonitor.settings.recordingStopped')

  const windowListLabel =
    windowHint === 'permission'
      ? t('screenMonitor.settings.windowListPermission')
      : windowHint === 'empty'
        ? t('screenMonitor.settings.windowListEmpty')
        : windowHint === 'ok'
          ? t('screenMonitor.settings.windowListOk')
          : t('screenMonitor.settings.windowListUnknown')

  const windowSectionHint =
    windowHint === 'permission'
      ? t('screenMonitor.settings.windowPermissionBlocked')
      : windowHint === 'empty'
        ? t('screenMonitor.settings.windowListEmpty')
        : t('screenMonitor.settings.onlyOpenedApps')

  return (
    <Modal
      title={t('common.settings')}
      visible={visible}
      autoFocus={false}
      focusLock
      onCancel={onCancel}
      className="text-[var(--color-text-4)]"
      unmountOnExit
      footer={
        <>
          <Button onClick={onCancel} className="[&_.arco-btn]: !text-xs">
            {t('common.cancel')}
          </Button>
          <Button
            type="primary"
            onClick={onSave}
            className="[&_.arco-btn-primary]: !bg-[rgb(var(--primary-6))] !border-[rgb(var(--primary-6))]">
            {t('common.save')}
          </Button>
        </>
      }
      style={{ width: 682 }}>
      <Form layout="vertical" form={form}>
        <div
          data-testid="capture-status-panel"
          className="mb-3 rounded-[8px] border border-[var(--color-border-2)] bg-[var(--color-fill-1)] px-3 py-2 text-[12px] leading-[18px] text-[var(--color-text-2)]">
          <div data-testid="capture-tcc-status">
            {t('screenMonitor.settings.tccLabel')}: {tccLabel}
          </div>
          <div data-testid="capture-recording-status">
            {t('screenMonitor.settings.recordingStateLabel')}: {recordingLabel}
          </div>
          <div data-testid="capture-window-list-status">
            {t('screenMonitor.settings.windowListLabel')}: {windowListLabel}
          </div>
          {showQuitRelaunch ? (
            <div data-testid="quit-relaunch-hint" className="mt-1 text-[var(--color-text-3)]">
              {t('screenMonitor.settings.quitRelaunchAfterTcc')}
            </div>
          ) : null}
          {(showTccDenied || showWindowPermission) && onRequestPermission ? (
            <div className="mt-2">
              <Button size="mini" type="primary" onClick={onRequestPermission}>
                {t('screenMonitor.settings.openScreenRecording')}
              </Button>
            </div>
          ) : null}
        </div>
        {showNotStarted ? (
          <Alert
            type="info"
            className="mb-3"
            data-testid="recording-not-started-alert"
            content={t('screenMonitor.settings.recordingNotStarted')}
          />
        ) : null}
        {showWindowPermission ? (
          <Alert
            type="warning"
            className="mb-3"
            data-testid="window-permission-alert"
            content={t('screenMonitor.settings.windowPermissionBlocked')}
            action={
              onRequestPermission ? (
                <Button size="mini" type="primary" onClick={onRequestPermission}>
                  {t('screenMonitor.settings.openScreenRecording')}
                </Button>
              ) : undefined
            }
          />
        ) : null}
        <div className="flex w-full flex-1 mt-5">
          <div className="flex flex-col flex-1 pr-[24px]">
            <Form.Item
              label={t('screenMonitor.settings.recordInterval')}
              className="[&_.arco-form-item-label]:!text-xs">
              <Slider
                value={tempRecordInterval}
                onChange={(value) => onSetTempRecordInterval(value as number)}
                min={5}
                max={300}
                marks={{
                  5: '5s',
                  300: '5min'
                }}
                className="!mt-4"
                formatTooltip={(value) => `${value}s`}
              />
            </Form.Item>
            <Form.Item label={t('screenMonitor.settings.chooseWhatToRecord')} shouldUpdate>
              {(values) => {
                const { screenSources = [], windowSources = [] } = values || {}
                const screenList = screenAllSources?.filter((source) => screenSources.includes(source.id)) || []
                const windowList = appAllSources?.filter((source) => windowSources.includes(source.id)) || []
                return (
                  <Spin loading={sources.state === 'loading'} block>
                    <Application
                      value={[...screenList, ...windowList]}
                      onCancel={() => onSetApplicationVisible(false)}
                      visible={applicationVisible}
                      onOk={() => onSetApplicationVisible(true)}
                    />
                  </Spin>
                )
              }}
            </Form.Item>
            <Form.Item
              label={t('screenMonitor.settings.enableRecordingHours')}
              className="[&_.arco-form-item-label]:!text-xs !mb-0">
              <Switch
                checked={tempEnableRecordingHours}
                onChange={onSetTempEnableRecordingHours}
                className={
                  !tempEnableRecordingHours
                    ? '[&_.arco-switch]: !bg-[var(--color-fill-2)]'
                    : '[&_.arco-switch]: !bg-[rgb(var(--primary-6))]'
                }
              />
            </Form.Item>
            {tempEnableRecordingHours && (
              <div className="!mt-3">
                <Form.Item
                  label={t('screenMonitor.settings.setRecordingHours')}
                  className="[&_.arco-form-item-label]:!text-xs">
                  <TimePicker.RangePicker
                    format="HH:mm"
                    value={tempRecordingHours}
                    onChange={(value) => onSetTempRecordingHours(value as [string, string])}
                  />
                </Form.Item>
                <Form.Item
                  label={t('screenMonitor.settings.applyToDays')}
                  className="[&_.arco-form-item-label]: !text-xs">
                  <Radio.Group value={tempApplyToDays} onChange={onSetTempApplyToDays}>
                    <Radio value="weekday" className="[&_.arco-radio-mask]: !border-[var(--color-border-2)]">
                      {t('screenMonitor.settings.onlyWeekday')}
                    </Radio>
                    <Radio value="everyday" className="[&_.arco-radio-mask]: !border-[var(--color-border-2)]">
                      {t('screenMonitor.settings.everyday')}
                    </Radio>
                  </Radio.Group>
                </Form.Item>
              </div>
            )}
          </div>
          <div
            className={clsx(
              'flex flex-col flex-1 border-l border-[var(--color-border-1)] max-h-[360px] h-[360px] overflow-x-hidden overflow-y-auto px-[16px]  [&_.arco-checkbox-checked_.arco-checkbox-mask]:!bg-[var(--color-text-1)] [&_.arco-checkbox-checked_.arco-checkbox-mask]:!border-[var(--color-text-1)]',
              { hidden: !applicationVisible }
            )}>
            <div className="text-[15px] leading-[18px] text-[var(--color-text-2)] mb-[12px] font-medium">
              {t('screenMonitor.settings.chooseWhatToRecord')}
            </div>
            <div className="[&_.arco-checkbox]:!flex [&_.arco-checkbox]:!items-center">
              <div className="text-[14px] leading-[20px] text-[var(--color-text-2)] mb-[4px]">
                {t('screenMonitor.settings.screenSection')}
              </div>
              <Form.Item field="screenSources">
                <Checkbox.Group className="!grid grid-cols-3 gap-4 relative [&_label]:!mr-0 [&_.arco-checkbox-text]:!ml-0">
                  {screenAllSources.map((source) => (
                    <Checkbox key={source.id} value={source.id}>
                      {({ checked }) => {
                        return (
                          <div className="flex flex-col items-center gap-[4px]">
                            <div
                              className={clsx(
                                'w-[94px] h-[60px] min-w-[94px] min-h-[60px] rounded-[8px] overflow-hidden border relative',
                                // 选中环用高对比语义色：写死黑色在深色主题下会与背景融为一体而看不出选中
                                checked ? 'border-[var(--color-text-1)]' : 'border-transparent'
                              )}>
                              {source.thumbnail ? (
                                <img
                                  src={source.thumbnail}
                                  alt="thumbnail"
                                  className="w-[94px] h-[60px] inline-block object-cover"
                                />
                              ) : (
                                <div className="w-[94px] h-[60px] flex items-center justify-center bg-[var(--color-fill-1)]">
                                  <img src={screenIcon} alt="" className="w-[28px] h-[28px] opacity-60" />
                                </div>
                              )}
                              <Checkbox checked={checked} className="!absolute !top-[4px] !right-[4px]" />
                            </div>
                            <div className="flex items-center space-x-[4px]">
                              {source.appIcon ? (
                                <img
                                  src={source.appIcon || ''}
                                  alt=""
                                  className="w-[14px] h-[14px] inline-block object-cover"
                                />
                              ) : (
                                <img src={screenIcon} alt="" className="w-[14px] h-[14px] inline-block object-cover" />
                              )}
                              <div className="text-[13px] leading-[22px] text-[var(--color-text-1)] !ml-[4px] line-clamp-1">
                                {source.name}
                              </div>
                            </div>
                          </div>
                        )
                      }}
                    </Checkbox>
                  ))}
                </Checkbox.Group>
              </Form.Item>
            </div>
            <div className="[&_.arco-checkbox]:!flex [&_.arco-checkbox]:!items-center">
              <div className="text-[14px] leading-[20px] text-[var(--color-text-2)] mb-[4px]">
                {t('screenMonitor.settings.windowSection')}
              </div>
              <div className="text-[10px] leading-[12px] text-[var(--color-text-3)] mb-[4px]">{windowSectionHint}</div>
              <Form.Item field="windowSources">
                <Checkbox.Group className="flex flex-col space-y-4">
                  {appAllSources.map((source) => {
                    const icon = source.appIcon || source.thumbnail
                    return (
                      <Checkbox key={source.id} value={source.id}>
                        <div className="flex items-center space-x-[4px]">
                          {icon ? (
                            <img src={icon} alt="" className="w-[14px] h-[14px] inline-block object-cover" />
                          ) : (
                            <span className="w-[14px] h-[14px] inline-block rounded-[3px] bg-[var(--color-fill-2)]" />
                          )}
                          <div className="text-[13px] leading-[22px] text-[var(--color-text-1)] !ml-[4px] line-clamp-1">
                            {source.name}
                          </div>
                        </div>
                      </Checkbox>
                    )
                  })}
                </Checkbox.Group>
              </Form.Item>
            </div>
          </div>
        </div>
      </Form>
    </Modal>
  )
}

export default SettingsModal
