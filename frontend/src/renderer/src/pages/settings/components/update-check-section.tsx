// 设置页：对照 GitHub Releases 检查更新，有新版本时打开发布页或 dmg。
// 不做静默安装；`api` 可注入便于页面级测试。

import { Button, Link, Typography } from '@arco-design/web-react'
import { createUpdateCheck, type UpdateCheck, type UpdateInfo } from '@renderer/adapters/update-check'
import { useI18n } from '@renderer/i18n'
import { useCallback, useState } from 'react'

const { Text } = Typography

export function UpdateCheckSection({ api }: { api?: UpdateCheck }) {
  const { t } = useI18n()
  const [shell] = useState(() => api ?? createUpdateCheck(globalThis))
  const [pending, setPending] = useState(false)
  const [note, setNote] = useState('')
  const [update, setUpdate] = useState<UpdateInfo | null>(null)
  const [currentVersion, setCurrentVersion] = useState('')

  const supported = shell.shell !== 'none'

  const onCheck = useCallback(async () => {
    setPending(true)
    setNote('')
    setUpdate(null)
    try {
      const result = await shell.check()
      setCurrentVersion(result.currentVersion)
      if (result.updateInfo) {
        setUpdate(result.updateInfo)
        setNote('')
      } else {
        setNote(t('settings.updateCheck.upToDate', { version: result.currentVersion }))
      }
    } catch {
      setNote(t('settings.updateCheck.failed'))
    } finally {
      setPending(false)
    }
  }, [shell, t])

  const open = useCallback(
    async (url: string) => {
      try {
        await shell.openUrl(url)
      } catch {
        setNote(t('settings.updateCheck.openFailed'))
      }
    },
    [shell, t]
  )

  return (
    <div className="flex max-w-[520px] flex-col gap-2 py-1" data-testid="settings-update-check">
      <div className="flex items-center justify-between gap-6">
        <div className="flex flex-col">
          <span className="text-[14px]">{t('settings.updateCheck')}</span>
          <Text type="secondary" className="!text-[12px]">
            {!supported
              ? t('settings.updateCheck.unsupported')
              : t('settings.updateCheck.hint')}
          </Text>
        </div>
        <Button
          size="small"
          type="outline"
          disabled={!supported || pending}
          loading={pending}
          onClick={() => void onCheck()}
          aria-label={t('settings.updateCheck')}
          data-testid="settings-update-check-button">
          {t('settings.updateCheck.action')}
        </Button>
      </div>
      {note ? (
        <Text type="secondary" className="!text-[12px]" data-testid="settings-update-check-note">
          {note}
        </Text>
      ) : null}
      {update ? (
        <div className="flex flex-col gap-1" data-testid="settings-update-check-available">
          <Text className="!text-[13px]">
            {t('settings.updateCheck.available', {
              latest: update.version,
              current: currentVersion || '—'
            })}
          </Text>
          <div className="flex flex-wrap gap-3 text-[12px]">
            <Link
              href={update.htmlUrl}
              onClick={(event) => {
                event.preventDefault()
                void open(update.htmlUrl)
              }}>
              {t('settings.updateCheck.openRelease')}
            </Link>
            {update.dmgUrl ? (
              <Link
                href={update.dmgUrl}
                onClick={(event) => {
                  event.preventDefault()
                  void open(update.dmgUrl as string)
                }}>
                {t('settings.updateCheck.openDmg')}
              </Link>
            ) : null}
          </div>
        </div>
      ) : null}
    </div>
  )
}
