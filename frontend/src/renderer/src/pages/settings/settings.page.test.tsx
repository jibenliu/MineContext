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
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { Provider } from 'react-redux'
import { beforeEach, expect, it, vi } from 'vitest'

import { getModelInfo, getStoredApiKey, ModelInfoResponseData } from '../../services/settings'
import { writeClipboard } from '../../utils/write-clipboard'
import Settings from './settings'

vi.mock('../../services/settings', async () => {
  const actual = await vi.importActual<typeof import('../../services/settings')>('../../services/settings')
  return { ...actual, getModelInfo: vi.fn(), getStoredApiKey: vi.fn() }
})

vi.mock('../../utils/write-clipboard', () => ({
  writeClipboard: vi.fn().mockResolvedValue(undefined)
}))

vi.mock('@arco-design/web-react', async () => {
  const actual = await vi.importActual<typeof import('@arco-design/web-react')>('@arco-design/web-react')
  return {
    ...actual,
    Message: {
      ...actual.Message,
      success: vi.fn(),
      error: vi.fn()
    }
  }
})

// 补推断区有 Arco DatePicker；全量页面套件里定时器偶发在卸载后触发，导致
// `window is not defined` 未捕获错误。本文件只钉模型平台表单，不测补推断。
vi.mock('./components/backfill-section', () => ({
  BackfillSection: () => null
}))

vi.mock('../../components/indexing-pause-banner', () => ({
  IndexingPauseBanner: () => null
}))

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
  vi.mocked(getStoredApiKey).mockResolvedValue('sk-live-secret-key-6789')
  vi.mocked(writeClipboard).mockClear()
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
  expect(screen.getByTestId('api-key-configured-hint')).toBeInTheDocument()
})

it('异步拉回已存密钥后，脱敏串以可见文本显示（不是 password 圆点）', async () => {
  let resolveInfo!: (value: ModelInfoResponseData) => void
  vi.mocked(getModelInfo).mockImplementation(
    () =>
      new Promise((resolve) => {
        resolveInfo = resolve
      })
  )
  renderSettings()

  resolveInfo(doubaoConfig)
  const input = await screen.findByDisplayValue('sk-l••••••••6789')
  expect(input).toHaveAttribute('type', 'text')
  expect(screen.getByTestId('api-key-configured-hint')).toBeInTheDocument()
})

it('自建平台视觉与向量密钥回填后都是可见脱敏串', async () => {
  vi.mocked(getModelInfo).mockResolvedValue({
    ...selfHostedConfig,
    hasApiKey: true,
    apiKeyMasked: 'sk-v••••••••ion1',
    hasEmbeddingApiKey: true,
    embeddingApiKeyMasked: 'sk-e••••••••bed1'
  })
  renderSettings()

  expect(await screen.findByText('视觉语言模型')).toBeInTheDocument()
  const vision = await screen.findByDisplayValue('sk-v••••••••ion1')
  const embedding = await screen.findByDisplayValue('sk-e••••••••bed1')
  expect(vision).toHaveAttribute('type', 'text')
  expect(embedding).toHaveAttribute('type', 'text')
})

it('点击复制时写入明文而非脱敏串', async () => {
  renderSettings()
  expect(await screen.findByText('复制')).toBeInTheDocument()
  fireEvent.click(screen.getByText('复制'))
  await waitFor(() => {
    expect(getStoredApiKey).toHaveBeenCalled()
    expect(writeClipboard).toHaveBeenCalledWith('sk-live-secret-key-6789')
  })
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

it('自建平台视觉与向量密钥都有复制入口，点复制写入明文', async () => {
  vi.mocked(getModelInfo).mockResolvedValue({
    ...selfHostedConfig,
    hasApiKey: true,
    apiKeyMasked: 'sk-v••••••••ion1',
    hasEmbeddingApiKey: true,
    embeddingApiKeyMasked: 'sk-e••••••••bed1'
  })
  vi.mocked(getStoredApiKey).mockImplementation(async (field = 'vision') =>
    field === 'embedding' ? 'sk-embed-plain-key-9999' : 'sk-vision-plain-key-1111'
  )
  renderSettings()

  expect(await screen.findByText('视觉语言模型')).toBeInTheDocument()
  const copyButtons = screen.getAllByText('复制')
  expect(copyButtons.length).toBeGreaterThanOrEqual(2)

  fireEvent.click(copyButtons[0])
  await waitFor(() => {
    expect(getStoredApiKey).toHaveBeenCalledWith('vision')
    expect(writeClipboard).toHaveBeenCalledWith('sk-vision-plain-key-1111')
  })

  vi.mocked(writeClipboard).mockClear()
  vi.mocked(getStoredApiKey).mockClear()
  fireEvent.click(copyButtons[1])
  await waitFor(() => {
    expect(getStoredApiKey).toHaveBeenCalledWith('embedding')
    expect(writeClipboard).toHaveBeenCalledWith('sk-embed-plain-key-9999')
  })
})

it('点眼睛显示密钥时换成明文，而不是脱敏串', async () => {
  renderSettings()
  const masked = await screen.findByDisplayValue('sk-l••••••••6789')
  const toggle = masked.closest('.arco-input-group-wrapper')?.querySelector('.arco-input-password-visibility-icon')
  expect(toggle).toBeTruthy()
  fireEvent.click(toggle as Element)

  await waitFor(() => {
    expect(getStoredApiKey).toHaveBeenCalled()
    expect(screen.getByDisplayValue('sk-live-secret-key-6789')).toBeInTheDocument()
  })
  expect(screen.queryByDisplayValue('sk-l••••••••6789')).not.toBeInTheDocument()
})

it('引导态设置页仍渲染模型表单与隐私出网入口（首屏不能空）', async () => {
  render(
    <Provider store={store}>
      <Settings init closeSetting={() => undefined} />
    </Provider>
  )

  expect(await screen.findByText('选择模型')).toBeInTheDocument()
  expect(screen.getByText('隐私与出网')).toBeInTheDocument()
  expect(screen.getByTestId('ai-upload-switch')).toBeInTheDocument()
})

it('引导态可点「稍后再说」离开，不依赖保存成功', async () => {
  const closeSetting = vi.fn()
  render(
    <Provider store={store}>
      <Settings init closeSetting={closeSetting} />
    </Provider>
  )

  fireEvent.click(await screen.findByTestId('settings-skip-onboarding'))
  expect(closeSetting).toHaveBeenCalledTimes(1)
})

it('非引导态不展示「稍后再说」', async () => {
  renderSettings()
  expect(await screen.findByText('选择模型')).toBeInTheDocument()
  expect(screen.queryByTestId('settings-skip-onboarding')).toBeNull()
})

it('引导态也会回填已存密钥（可沿用密钥点开始使用，不必重填）', async () => {
  render(
    <Provider store={store}>
      <Settings init closeSetting={() => undefined} />
    </Provider>
  )

  expect(await screen.findByDisplayValue('sk-l••••••••6789')).toBeInTheDocument()
  expect(screen.getByText(/已保存密钥/)).toBeInTheDocument()
})

it('引导态加载中不盖遮罩：开始使用按钮仍可点', async () => {
  // getModelInfo 挂起时整页 loading mask 不得吞掉 CTA 点击（否则无日志、无反馈）。
  vi.mocked(getModelInfo).mockImplementation(() => new Promise(() => undefined))
  render(
    <Provider store={store}>
      <Settings init closeSetting={() => undefined} />
    </Provider>
  )

  expect(await screen.findByTestId('settings-loading-hint')).toBeInTheDocument()
  const submit = screen.getByTestId('settings-submit')
  expect(submit).toBeEnabled()
  fireEvent.click(submit)
  // 点击进了 handler：校验失败会 Toast「保存设置失败」或字段「不能为空」（二者任一即可）。
  await waitFor(() => {
    const feedback =
      screen.queryByText('不能为空') ||
      screen.queryByText('保存设置失败') ||
      document.body.textContent?.includes('不能为空')
    expect(feedback).toBeTruthy()
  })
})
