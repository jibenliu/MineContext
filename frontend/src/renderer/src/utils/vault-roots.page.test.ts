import type { VaultTreeNode } from '@renderer/types'
import { describe, expect, it } from 'vitest'

import { getVaultRoots } from './vault'

function node(partial: Partial<VaultTreeNode> & Pick<VaultTreeNode, 'id' | 'title'>): VaultTreeNode {
  return {
    is_folder: 0,
    children: [],
    ...partial
  } as VaultTreeNode
}

describe('getVaultRoots', () => {
  it('returns only top-level folders under the virtual root', () => {
    const tree = node({
      id: -1,
      title: 'root',
      is_folder: 1,
      children: [
        node({ id: 1, title: 'Work', is_folder: 1, children: [node({ id: 11, title: 'note', is_folder: 0 })] }),
        node({ id: 2, title: 'Personal', is_folder: 1 }),
        node({ id: 3, title: 'loose note', is_folder: 0 })
      ]
    })

    const roots = getVaultRoots(tree)
    expect(roots.map((root) => root.id)).toEqual([1, 2])
  })
})
