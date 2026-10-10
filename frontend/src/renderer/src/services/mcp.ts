import axiosInstance from '@renderer/services/axios-config'
import { get } from 'lodash'

export interface McpServer {
  id: string
  name: string
  enabled: boolean
  transport: 'stdio' | 'http'
  command?: string | null
  args: string[]
  cwd?: string | null
  url?: string | null
  env_refs: string[]
  allowed_tools: string[]
  requires_network: boolean
}

export interface McpSettings {
  enabled: boolean
  servers: McpServer[]
  ai_upload: boolean
}

export interface McpTool {
  server_id: string
  name: string
  description: string
  input_schema: unknown
}

export async function getMcpSettings(): Promise<McpSettings> {
  const res = await axiosInstance.get<McpSettings>('/api/mcp')
  const data = get(res, 'data.data') as McpSettings | undefined
  return {
    enabled: Boolean(data?.enabled),
    servers: Array.isArray(data?.servers) ? data!.servers : [],
    ai_upload: Boolean(data?.ai_upload)
  }
}

export async function setMcpEnabled(enabled: boolean): Promise<McpSettings> {
  const res = await axiosInstance.put<McpSettings>('/api/mcp', { enabled })
  const data = get(res, 'data.data') as McpSettings | undefined
  return {
    enabled: Boolean(data?.enabled),
    servers: Array.isArray(data?.servers) ? data!.servers : [],
    ai_upload: Boolean(data?.ai_upload)
  }
}

export async function upsertMcpServer(body: {
  id: string
  name?: string
  enabled?: boolean
  transport?: 'stdio' | 'http'
  command?: string
  args?: string[]
  url?: string
  env?: Record<string, string>
  allowed_tools?: string[]
  requires_network?: boolean
}): Promise<McpSettings> {
  const res = await axiosInstance.post<McpSettings>('/api/mcp/servers', body)
  const data = get(res, 'data.data') as McpSettings | undefined
  return {
    enabled: Boolean(data?.enabled),
    servers: Array.isArray(data?.servers) ? data!.servers : [],
    ai_upload: Boolean(data?.ai_upload)
  }
}

export async function deleteMcpServer(id: string): Promise<McpSettings> {
  const res = await axiosInstance.delete<McpSettings>(`/api/mcp/servers/${encodeURIComponent(id)}`)
  const data = get(res, 'data.data') as McpSettings | undefined
  return {
    enabled: Boolean(data?.enabled),
    servers: Array.isArray(data?.servers) ? data!.servers : [],
    ai_upload: Boolean(data?.ai_upload)
  }
}

export async function listMcpTools(): Promise<McpTool[]> {
  const res = await axiosInstance.get<McpTool[]>('/api/mcp/tools')
  const data = get(res, 'data.data')
  return Array.isArray(data) ? (data as McpTool[]) : []
}

export async function reloadMcp(): Promise<void> {
  await axiosInstance.post('/api/mcp/reload')
}
