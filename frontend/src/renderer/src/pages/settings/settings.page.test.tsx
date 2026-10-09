// 设置页：模型平台决定渲染哪个平台的表单，而**后端不存平台名**
// （`config.toml` 里只有 base_url 与模型名，接口返回的 `modelPlatform` 是空串）。
//
// 这条边界以前把表单整个渲染没了：空串匹配不上任何一个平台分支，渲染函数返回
// null —— 界面上「模型平台」下面直接就是保存按钮，与「功能没做」难以区分。
//
// 所以这里钉两类行为：
//   1. 后端给了 base_url 就要落到对应平台（OpenAI / 自建），不能停在默认平台；
//   2. 任何情况下都必须渲染出某个平台的表单，不允许空白。

import store from '@renderer/store'
import { render, screen } from '@testing-library/react'
import { Provider } from 'react-redux'
import { beforeEach, expect, it, vi } from 'vitest'

import { getModelInfo, ModelInfoResponseData } from '../../services/settings'
import Settings from './settings'

vi.mock('../../services/settings', async () => {
  const actual = await vi.importActual<typeof import('../../services/settings')>('../../services/settings')
  return { ...actual, getModelInfo: vi.fn() }
})

const doubaoConfig: ModelInfoResponseData = {
  config: {
    modelPlatform: '',
    modelId: 'doubao-seed-1-6-flash-250828',
    baseUrl: 'https://ark.cn-beijing.volces.com/api/v3',
    apiKey: '',
    embeddingModelId: 'doubao-embedding-vision-250615',
    embeddingBaseUrl: 'https://ark.cn-beijing.volces.com/api/v3',
    embeddingApiKey: ''
  },
  hasApiKey: true,
  apiKeyMasked: 'sk-l••••••••6789'
}

const openaiConfig: ModelInfoResponseData = {
  config: {
    modelPlatform: '',
    modelId: 'gpt-5',
    baseUrl: 'https://api.openai.com/v1',
    apiKey: '',
    embeddingModelId: 'text-embedding-3-large',
    embeddingBaseUrl: 'https://api.openai.com/v1',
    embeddingApiKey: ''
  }
}

const selfHostedConfig: ModelInfoResponseData = {
  config: {
    modelPlatform: '',
    modelId: 'my-vlm',
    baseUrl: 'http://192.168.1.9:8000/v1',
    apiKey: '',
    embeddingModelId: 'my-embedding',
    embeddingBaseUrl: 'http://192.168.1.9:8000/v1',
    embeddingApiKey: ''
  }
}

function renderSettings() {
  return render(
    <Provider store={store}>
      <Settings />
    </Provider>
  )
}

beforeEach(() => {
  vi.mocked(getModelInfo).mockResolvedValue(doubaoConfig)
})

it('默认平台是 Doubao，模型选择与 API Key 输入框都在', async () => {
  renderSettings()

  expect(await screen.findByText('选择模型')).toBeInTheDocument()
  expect(screen.getByText('API Key')).toBeInTheDocument()
  expect(screen.getByText('获取豆包 API Key')).toBeInTheDocument()
})

it('已保存密钥时脱敏回显，并提供复制入口', async () => {
  renderSettings()

  expect(await screen.findByDisplayValue('sk-l••••••••6789')).toBeInTheDocument()
  expect(screen.getByText('复制')).toBeInTheDocument()
  expect(screen.getByText(/已保存密钥/)).toBeInTheDocument()
})

it('后端 base_url 是 OpenAI 时落到 OpenAI 平台，表单不空', async () => {
  vi.mocked(getModelInfo).mockResolvedValue(openaiConfig)
  renderSettings()

  expect(await screen.findByText('获取 OpenAI API Key')).toBeInTheDocument()
  expect(screen.getByText('选择模型')).toBeInTheDocument()
  expect(screen.getByText('API Key')).toBeInTheDocument()
})

it('后端 base_url 既不是豆包也不是 OpenAI 时落到自建平台，表单不空', async () => {
  vi.mocked(getModelInfo).mockResolvedValue(selfHostedConfig)
  renderSettings()

  expect(await screen.findByText('视觉语言模型')).toBeInTheDocument()
  expect(screen.queryByText('选择模型')).not.toBeInTheDocument()
})
