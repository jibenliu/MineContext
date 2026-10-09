// 页面级测试：笔记页能渲染出 store 里的那条笔记。
//
// 笔记页此前没有任何测试：238 行的文件页与 152 行的它，是页面层最后两块空白。
// 与其它页面不同，笔记页的内容**不直接来自渠道**：它从 `useVaults().findVaultById(id)` 取，
// 也就是 redux 的 vault 切片，因此要点是「把 store 喂上」+「router 给出 ?id=1」。
//
// 编辑器（crepe）被替换成桩：它是独立关注点，页面测试不该替「编辑器能不能在 jsdom 里挂载」
// 背书 —— 这里要验的是页面把标题与正文接到了编辑器上。

import store from '@renderer/store'
import { installFakeBackend } from '@renderer/test/page-setup'
import { render, screen } from '@testing-library/react'
import { Provider } from 'react-redux'
import { MemoryRouter } from 'react-router-dom'
import { describe, expect, it, vi } from 'vitest'

import VaultPage from './vault'

vi.mock('@renderer/components/markdown-editor', () => ({
  default: ({ defaultValue }: { defaultValue: string }) => <div>{defaultValue}</div>
}))

/** 形状对齐 `vaults` 表（笔记树与笔记编辑器直接读这些列）。 */
const VAULT = {
  id: 1,
  title: '季度复盘',
  content: '正文内容',
  summary: '',
  tags: '',
  parent_id: null,
  is_folder: 0,
  is_deleted: 0,
  created_at: '2026-10-08 09:00:00',
  updated_at: '2026-10-08 09:00:00',
  sort_order: 0
}

describe('笔记页（渲染 + store 接线）', () => {
  it('把 store 里那条笔记的标题与正文交给编辑器', async () => {
    // 渠道清单与 `vault-tree.page.test.tsx` 一致：store 里的 vault 列表靠它们填充
    // store 里的 vault 列表由 `getAllVaults()` 填充，它读的是
    // `database:get-vaults-by-document-type`（类型取 DailyReport + vaults）
    installFakeBackend(
      {
        'database:get-vaults-by-document-type': [VAULT],
        'database:get-all-vaults': [VAULT]
      },
      { strict: false }
    )

    render(
      <Provider store={store}>
        <MemoryRouter initialEntries={['/vault?id=1']}>
          <VaultPage />
        </MemoryRouter>
      </Provider>
    )

    // 标题编辑器拿到的 `title` 是 `'## ' + vault.title`
    expect(await screen.findByText(/季度复盘/)).toBeInTheDocument()
    expect(await screen.findByText('正文内容')).toBeInTheDocument()
  })

  it('找不到笔记时给出说明而不是永远转圈', async () => {
    installFakeBackend(
      {
        'database:get-vaults-by-document-type': [VAULT],
        'database:get-all-vaults': [VAULT]
      },
      { strict: false }
    )

    render(
      <Provider store={store}>
        <MemoryRouter initialEntries={['/vault?id=999']}>
          <VaultPage />
        </MemoryRouter>
      </Provider>
    )

    expect(await screen.findByText(/找不到这篇笔记/)).toBeInTheDocument()
    expect(screen.getByRole('link', { name: /返回首页/ })).toBeInTheDocument()
  })
})
