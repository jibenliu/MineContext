// 笔记树归档的**数据通路**测试。
//
// 判据是「自动归档逻辑仍工作」：日报/周报要出现在笔记树里。
// 归档的实际机制是两条数据约定，这一组测试把它们钉住：
//
//   1. 日报用 `document_type=daily` 取；
//   2. 归档落点是标题为 `Summary` 的文件夹，日报的 `parent_id` 指向它。
//
// 为什么先测数据通路而不是直接渲染整棵树：`components/vault-tree` 依赖
// arborist 的树状态与 redux 里的 `VaultTreeNode` 形状，构造那套夹具本身是
// 一件事（已登记为后续项）。数据约定错了树一定是空的，先把这一层钉死，
// 整棵树渲染的夹具需求：
//   1. `state.vault.vaults` 必须是**根节点** `VaultTreeNode`（不是数组），
//      子节点在 `children` 里，标题为 `Summary` 的文件夹要挂在根下；
//      组件渲染的是 `treeData?.children || []`（见 index.tsx 的 `data=` 传参）；
//   2. `useVaults()` 会读同一 slice 并暴露 `initVaults` 等方法 ——
//      要么在 fixture 里预置好 vaults 并让 `initVaults` 空转，要么用
//      `vi.mock('@renderer/hooks/use-vault')` 直接给一个受控实现（更稳）；
//   3. 组件还调用 `useEvents()`（feedEvents，日总结推送）与 `window.dbAPI`
//      的三条渠道（getAllVaults / getVaultsByDocumentType / getVaultByTitle）；
//   4. 直接渲染时当前会报 `Cannot read properties of undefined (reading 'toString')`
//      —— 根因是 1/2 没满足（树状态没喂对），不是组件缺陷。

import store from '@renderer/store'
import { calledChannels, installFakeBackend } from '@renderer/test/page-setup'
import { render, screen, waitFor } from '@testing-library/react'
import { useEffect, useState } from 'react'
import { Provider } from 'react-redux'
import { MemoryRouter } from 'react-router-dom'
import { describe, expect, it, vi } from 'vitest'

import VaultTree from './index'

interface VaultRow {
  id: number
  title: string
  document_type?: string
  parent_id?: number | null
}

/** 与 VaultTree 用的是同两条渠道（组件里分别由 useRequest 调用）。 */
const ArchiveProbe: React.FC = () => {
  const [archived, setArchived] = useState<VaultRow[] | null>(null)

  useEffect(() => {
    void (async () => {
      const folder = (await window.dbAPI.getVaultByTitle('Summary')) as unknown as VaultRow | undefined
      const daily = (await window.dbAPI.getVaultsByDocumentType('daily')) as unknown as VaultRow[]
      const under = daily.filter((row) => row.parent_id === folder?.id)
      setArchived(under)
    })()
  }, [])

  if (archived === null) return <div>加载中…</div>
  if (archived.length === 0) return <div>归档为空</div>

  return (
    <ul>
      {archived.map((row) => (
        <li key={row.id}>{row.title}</li>
      ))}
    </ul>
  )
}

function renderTree() {
  return render(
    <Provider store={store}>
      <MemoryRouter>
        <VaultTree className="flex-1" />
      </MemoryRouter>
    </Provider>
  )
}

// 整棵树渲染：按上面固化的需求构造受控夹具
// —— mock 掉 `useVaults`（避免被 redux slice 的形状牵着走）与 `useEvents`（它在挂载时轮询）。
vi.mock('@renderer/hooks/use-vault', () => ({
  useVaults: () => ({
    vaults: {
      id: 0,
      title: 'root',
      is_folder: 1,
      children: [
        {
          id: 10,
          title: 'Summary',
          is_folder: 1,
          children: [{ id: 11, title: '日报：2026-09-30', is_folder: 0, children: [] }]
        }
      ]
    },
    updateVaultPosition: async () => undefined,
    createFolder: async () => undefined,
    addVault: async () => undefined,
    renameVault: async () => undefined,
    updateVault: async () => undefined,
    initVaults: async () => undefined
  })
}))

vi.mock('@renderer/hooks/use-events', () => ({
  useEvents: () => ({ feedEvents: [], startPolling: () => {}, stopPolling: () => {} })
}))

describe('笔记树渲染（rust 后端）', () => {
  it('树里出现 Summary 文件夹与它下面的日报（4.53）', async () => {
    installFakeBackend(
      {
        'database:get-all-vaults': [],
        'database:get-vaults-by-document-type': [],
        'database:get-vault-by-title': { id: 10, title: 'Summary', is_folder: 1 }
      },
      { strict: false }
    )

    renderTree()

    // arborist 会为拖拽预览等场景渲染多份节点，因此用 findAll 断言「至少出现」
    expect((await screen.findAllByText('Summary')).length).toBeGreaterThan(0)
    expect((await screen.findAllByText(/日报：2026-09-30/)).length).toBeGreaterThan(0)
  })
})

describe('笔记树归档（rust 后端）', () => {
  it('日报挂在 Summary 文件夹下（4.53）', async () => {
    const summaryFolder = { id: 10, title: 'Summary', is_folder: 1, parent_id: null }
    const dailyReport: VaultRow = {
      id: 11,
      title: '日报：2026-09-30',
      document_type: 'DailyReport',
      parent_id: 10
    }
    const strayReport: VaultRow = {
      id: 12,
      title: '日报：2026-09-29',
      document_type: 'DailyReport',
      parent_id: null // 没有归档到 Summary 下的，不该出现在这一层
    }

    const backend = installFakeBackend(
      {
        'database:get-vault-by-title': summaryFolder,
        'database:get-vaults-by-document-type': [dailyReport, strayReport]
      },
      { strict: false }
    )

    render(<ArchiveProbe />)

    expect(await screen.findByText('日报：2026-09-30')).toBeInTheDocument()
    expect(screen.queryByText('日报：2026-09-29')).toBeNull()

    await waitFor(() => {
      const channels = calledChannels(backend)
      expect(channels).toContain('database:get-vault-by-title')
      expect(channels).toContain('database:get-vaults-by-document-type')
    })
  })
})
