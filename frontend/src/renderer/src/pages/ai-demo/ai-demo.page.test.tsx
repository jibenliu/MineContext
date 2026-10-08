import { render, screen } from '@testing-library/react'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { expect, it } from 'vitest'

import AIDemo from './ai-demo'

it('旧演示地址跳转真实助手，不再加载硬编码模型端点', async () => {
  render(
    <MemoryRouter initialEntries={['/ai-demo']}>
      <Routes>
        <Route path="/ai-demo" element={<AIDemo />} />
        <Route path="/assistant" element={<div>真实助手</div>} />
      </Routes>
    </MemoryRouter>
  )
  expect(await screen.findByText('真实助手')).toBeInTheDocument()
})
