// 设置页：MCP 插件启停、server 配置、工具白名单与可见工具列表。

import { Button, Input, Message, Select, Switch, Typography } from '@arco-design/web-react'
import { useI18n } from '@renderer/i18n'
import {
  deleteMcpServer,
  getMcpSettings,
  listMcpTools,
  McpServer,
  McpTool,
  reloadMcp,
  setMcpEnabled,
  upsertMcpServer
} from '@renderer/services/mcp'
import { useCallback, useEffect, useState } from 'react'

const { Text } = Typography

export function McpSection() {
  const { t } = useI18n()
  const [enabled, setEnabled] = useState(false)
  const [servers, setServers] = useState<McpServer[]>([])
  const [tools, setTools] = useState<McpTool[]>([])
  const [pending, setPending] = useState(true)
  const [formId, setFormId] = useState('')
  const [formName, setFormName] = useState('')
  const [formTransport, setFormTransport] = useState<'stdio' | 'http'>('stdio')
  const [formCommand, setFormCommand] = useState('')
  const [formUrl, setFormUrl] = useState('')
  const [formAllowed, setFormAllowed] = useState('search')
  const [formEnvRef, setFormEnvRef] = useState('')

  const refresh = useCallback(async () => {
    const settings = await getMcpSettings()
    setEnabled(settings.enabled)
    setServers(settings.servers)
    if (settings.enabled) {
      try {
        setTools(await listMcpTools())
      } catch {
        setTools([])
      }
    } else {
      setTools([])
    }
  }, [])

  useEffect(() => {
    let alive = true
    void (async () => {
      try {
        await refresh()
      } catch {
        // 读失败保持默认关
      } finally {
        if (alive) setPending(false)
      }
    })()
    return () => {
      alive = false
    }
  }, [refresh])

  const onToggle = useCallback(
    async (next: boolean) => {
      setPending(true)
      try {
        const settings = await setMcpEnabled(next)
        setEnabled(settings.enabled)
        setServers(settings.servers)
        Message.success(next ? t('settings.mcp.enabled') : t('settings.mcp.disabled'))
        if (next) {
          await reloadMcp().catch(() => undefined)
          setTools(await listMcpTools().catch(() => []))
        } else {
          setTools([])
        }
      } catch {
        Message.error(t('settings.mcp.failed'))
      } finally {
        setPending(false)
      }
    },
    [t]
  )

  const onSaveServer = useCallback(async () => {
    if (!formId.trim()) {
      Message.error(t('settings.mcp.idRequired'))
      return
    }
    setPending(true)
    try {
      const allowed = formAllowed
        .split(/[,\s]+/)
        .map((s) => s.trim())
        .filter(Boolean)
      const env: Record<string, string> = {}
      if (formEnvRef.trim()) {
        env.MCP_AUTH_TOKEN = formEnvRef.trim()
      }
      const settings = await upsertMcpServer({
        id: formId.trim(),
        name: formName.trim() || formId.trim(),
        enabled: true,
        transport: formTransport,
        command: formTransport === 'stdio' ? formCommand.trim() : undefined,
        url: formTransport === 'http' ? formUrl.trim() : undefined,
        allowed_tools: allowed,
        env: Object.keys(env).length ? env : undefined,
        requires_network: formTransport === 'http'
      })
      setEnabled(settings.enabled)
      setServers(settings.servers)
      Message.success(t('settings.mcp.serverSaved'))
      await reloadMcp().catch(() => undefined)
      setTools(await listMcpTools().catch(() => []))
    } catch {
      Message.error(t('settings.mcp.failed'))
    } finally {
      setPending(false)
    }
  }, [formAllowed, formCommand, formEnvRef, formId, formName, formTransport, formUrl, t])

  const onToggleServer = useCallback(
    async (server: McpServer, next: boolean) => {
      setPending(true)
      try {
        const settings = await upsertMcpServer({ id: server.id, enabled: next })
        setServers(settings.servers)
        await reloadMcp().catch(() => undefined)
        setTools(await listMcpTools().catch(() => []))
      } catch {
        Message.error(t('settings.mcp.failed'))
      } finally {
        setPending(false)
      }
    },
    [t]
  )

  const onRemove = useCallback(
    async (id: string) => {
      setPending(true)
      try {
        const settings = await deleteMcpServer(id)
        setServers(settings.servers)
        setTools(await listMcpTools().catch(() => []))
        Message.success(t('settings.mcp.serverRemoved'))
      } catch {
        Message.error(t('settings.mcp.failed'))
      } finally {
        setPending(false)
      }
    },
    [t]
  )

  return (
    <div className="flex max-w-[640px] flex-col gap-4 py-1" data-testid="mcp-section">
      <div className="flex items-center justify-between gap-6">
        <div className="flex flex-col">
          <span className="text-[14px]">{t('settings.mcp')}</span>
          <Text type="secondary" className="!text-[12px]">
            {t('settings.mcp.hint')}
          </Text>
        </div>
        <Switch
          checked={enabled}
          disabled={pending}
          onChange={onToggle}
          aria-label={t('settings.mcp')}
          data-testid="mcp-master-switch"
        />
      </div>

      {enabled ? (
        <>
          <div className="flex flex-col gap-2" data-testid="mcp-add-form">
            <Text className="!text-[13px] font-medium text-[var(--color-text-1)]">{t('settings.mcp.addServer')}</Text>
            <Input
              size="small"
              placeholder={t('settings.mcp.idPlaceholder')}
              value={formId}
              onChange={setFormId}
              data-testid="mcp-form-id"
            />
            <Input
              size="small"
              placeholder={t('settings.mcp.namePlaceholder')}
              value={formName}
              onChange={setFormName}
            />
            <Select
              size="small"
              value={formTransport}
              onChange={(v) => setFormTransport(v as 'stdio' | 'http')}
              options={[
                { label: 'stdio', value: 'stdio' },
                { label: 'http', value: 'http' }
              ]}
              data-testid="mcp-form-transport"
            />
            {formTransport === 'stdio' ? (
              <Input
                size="small"
                placeholder={t('settings.mcp.commandPlaceholder')}
                value={formCommand}
                onChange={setFormCommand}
                data-testid="mcp-form-command"
              />
            ) : (
              <Input
                size="small"
                placeholder={t('settings.mcp.urlPlaceholder')}
                value={formUrl}
                onChange={setFormUrl}
                data-testid="mcp-form-url"
              />
            )}
            <Input
              size="small"
              placeholder={t('settings.mcp.allowedPlaceholder')}
              value={formAllowed}
              onChange={setFormAllowed}
              data-testid="mcp-form-allowed"
            />
            <Input
              size="small"
              placeholder={t('settings.mcp.envRefPlaceholder')}
              value={formEnvRef}
              onChange={setFormEnvRef}
              data-testid="mcp-form-env"
            />
            <Button type="secondary" size="small" disabled={pending} onClick={() => void onSaveServer()} data-testid="mcp-save-server">
              {t('settings.mcp.saveServer')}
            </Button>
          </div>

          <div className="flex flex-col gap-2" data-testid="mcp-server-list">
            {servers.length === 0 ? (
              <Text type="secondary" className="!text-[12px]">
                {t('settings.mcp.noServers')}
              </Text>
            ) : (
              servers.map((server) => (
                <div
                  key={server.id}
                  className="flex items-start justify-between gap-3 border-b border-[var(--color-border-2)] py-2"
                  data-testid={`mcp-server-${server.id}`}>
                  <div className="flex min-w-0 flex-col">
                    <span className="truncate text-[13px] text-[var(--color-text-1)]">
                      {server.name || server.id}
                    </span>
                    <Text type="secondary" className="!text-[12px]">
                      {server.transport}
                      {server.allowed_tools.length
                        ? ` · ${server.allowed_tools.join(', ')}`
                        : ` · ${t('settings.mcp.noToolsAllowed')}`}
                      {server.requires_network ? ` · ${t('settings.mcp.needsNetwork')}` : ''}
                    </Text>
                  </div>
                  <div className="flex shrink-0 items-center gap-2">
                    <Switch
                      size="small"
                      checked={server.enabled}
                      disabled={pending}
                      onChange={(next) => void onToggleServer(server, next)}
                      aria-label={server.id}
                    />
                    <Button type="text" size="mini" status="danger" onClick={() => void onRemove(server.id)}>
                      {t('settings.mcp.remove')}
                    </Button>
                  </div>
                </div>
              ))
            )}
          </div>

          <div className="flex flex-col gap-1" data-testid="mcp-tool-list">
            <Text className="!text-[13px] font-medium text-[var(--color-text-1)]">{t('settings.mcp.allowedTools')}</Text>
            {tools.length === 0 ? (
              <Text type="secondary" className="!text-[12px]">
                {t('settings.mcp.noVisibleTools')}
              </Text>
            ) : (
              tools.map((tool) => (
                <Text key={`${tool.server_id}__${tool.name}`} className="!text-[12px]" type="secondary">
                  {tool.server_id}__{tool.name}
                  {tool.description ? ` — ${tool.description}` : ''}
                </Text>
              ))
            )}
          </div>
        </>
      ) : null}
    </div>
  )
}
