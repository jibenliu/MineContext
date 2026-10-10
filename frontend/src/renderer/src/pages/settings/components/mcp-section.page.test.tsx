import {
  deleteMcpServer,
  getMcpSettings,
  listMcpTools,
  reloadMcp,
  setMcpEnabled,
  upsertMcpServer
} from '@renderer/services/mcp'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, expect, it, vi } from 'vitest'

import { McpSection } from './mcp-section'

vi.mock('@renderer/services/mcp', () => ({
  getMcpSettings: vi.fn(),
  setMcpEnabled: vi.fn(),
  upsertMcpServer: vi.fn(),
  deleteMcpServer: vi.fn(),
  listMcpTools: vi.fn(),
  reloadMcp: vi.fn()
}))

vi.mock('@arco-design/web-react', async () => {
  const actual = await vi.importActual<typeof import('@arco-design/web-react')>('@arco-design/web-react')
  return {
    ...actual,
    Message: { ...actual.Message, success: vi.fn(), error: vi.fn() }
  }
})

beforeEach(() => {
  vi.mocked(getMcpSettings).mockResolvedValue({ enabled: false, servers: [], ai_upload: false })
  vi.mocked(setMcpEnabled).mockResolvedValue({ enabled: true, servers: [], ai_upload: false })
  vi.mocked(listMcpTools).mockResolvedValue([])
  vi.mocked(reloadMcp).mockResolvedValue(undefined)
  vi.mocked(upsertMcpServer).mockResolvedValue({
    enabled: true,
    ai_upload: false,
    servers: [
      {
        id: 'wiki',
        name: 'Wiki',
        enabled: true,
        transport: 'stdio',
        args: [],
        env_refs: [],
        allowed_tools: ['search'],
        requires_network: false
      }
    ]
  })
  vi.mocked(deleteMcpServer).mockResolvedValue({ enabled: true, servers: [], ai_upload: false })
})

it('启用 MCP 后可添加服务器并看到工具列表入口', async () => {
  render(<McpSection />)
  const toggle = await screen.findByTestId('mcp-master-switch')
  fireEvent.click(toggle)
  await waitFor(() => {
    expect(setMcpEnabled).toHaveBeenCalledWith(true)
  })

  await screen.findByTestId('mcp-add-form')
  const idHost = screen.getByTestId('mcp-form-id')
  const commandHost = screen.getByTestId('mcp-form-command')
  const idInput = (idHost.matches('input') ? idHost : idHost.querySelector('input')) as HTMLInputElement
  const commandInput = (
    commandHost.matches('input') ? commandHost : commandHost.querySelector('input')
  ) as HTMLInputElement
  fireEvent.change(idInput, { target: { value: 'wiki' } })
  fireEvent.change(commandInput, { target: { value: 'npx' } })
  fireEvent.click(screen.getByTestId('mcp-save-server'))

  await waitFor(() => {
    expect(upsertMcpServer).toHaveBeenCalledWith(
      expect.objectContaining({
        id: 'wiki',
        transport: 'stdio',
        command: 'npx',
        allowed_tools: ['search']
      })
    )
  })
  expect(await screen.findByTestId('mcp-tool-list')).toBeInTheDocument()
})
