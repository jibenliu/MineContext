// 设置页：开机自启开关。三条外壳分支的显示规则都收敛在这里
// （分支行为见 adapters/launch-at-login.ts）。`api` 可注入，便于页面级测试。

import { Switch, Typography } from '@arco-design/web-react'
import { createLaunchAtLogin, type LaunchAtLogin } from '@renderer/adapters/launch-at-login'
import { useI18n } from '@renderer/i18n'
import { useCallback, useEffect, useState } from 'react'

const { Text } = Typography

export function LaunchAtLoginSwitch({ api }: { api?: LaunchAtLogin }) {
  const { t } = useI18n()
  const [shell] = useState(() => api ?? createLaunchAtLogin(globalThis))
  const [enabled, setEnabled] = useState(false)
  const [pending, setPending] = useState(true)
  const [note, setNote] = useState('')

  useEffect(() => {
    let alive = true
    void (async () => {
      try {
        const value = await shell.read()
        if (alive) setEnabled(value ?? false)
      } catch (error) {
        if (alive) setNote(t('settings.launchAtLogin.unconfirmed'))
      } finally {
        if (alive) setPending(false)
      }
    })()
    return () => {
      alive = false
    }
  }, [shell, t])

  const onChange = useCallback(
    async (next: boolean) => {
      setPending(true)
      try {
        const readBack = await shell.write(next)
        // 外壳读不回真值时就照实说「未确认」，不假装知道系统状态
        setEnabled(readBack ?? next)
        setNote(readBack === undefined ? t('settings.launchAtLogin.unconfirmed') : '')
      } catch (error) {
        setNote(t('settings.launchAtLogin.unconfirmed'))
      } finally {
        setPending(false)
      }
    },
    [shell, t]
  )

  const supported = shell.shell !== 'none'

  return (
    <div className="flex max-w-[520px] items-center justify-between gap-6 py-1">
      <div className="flex flex-col">
        <span className="text-[14px]">{t('settings.launchAtLogin')}</span>
        <Text type="secondary" className="!text-[12px]">
          {!supported ? t('settings.launchAtLogin.unsupported') : (note ?? t('settings.launchAtLogin.hint'))}
        </Text>
      </div>
      <Switch checked={enabled} disabled={!supported || pending} onChange={onChange} aria-label="Launch at login" />
    </div>
  )
}
