// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import { ReactNode } from 'react'

import custom from '../../assets/images/settings/custom.svg'
import doubao from '../../assets/images/settings/doubao.png'
import openAI from '../../assets/images/settings/open-ai.png'

export enum ModelTypeList {
  Doubao = 'doubao',
  OpenAI = 'openai',
  Custom = 'custom'
}

export enum embeddingModels {
  DoubaoEmbeddingModelId = 'doubao-embedding-vision-250615',
  OpenAIEmbeddingModelId = 'text-embedding-3-large'
}
export enum BaseUrl {
  DoubaoUrl = 'https://ark.cn-beijing.volces.com/api/v3',
  OpenAIUrl = 'https://api.openai.com/v1'
}

const ModelPlatforms: string[] = [ModelTypeList.Doubao, ModelTypeList.OpenAI, ModelTypeList.Custom]

/** 平台名是否是可识别的枚举值（后端可能给空串或别的平台名）。 */
export function isKnownModelPlatform(value: unknown): value is ModelTypeList {
  return typeof value === 'string' && ModelPlatforms.includes(value)
}

/**
 * 判断当前配置属于哪个平台。
 *
 * 平台名**不落配置**：`config.toml` 里只有 `base_url` 与模型名，接口回的
 * `modelPlatform` 是空串。所以按 `base_url` 反推该显示哪个平台 —— 认不出来
 * （自建中转、内网地址）就归到 Custom，那里本来就允许填任意端点。
 * 认不出来时**不能返回空值**：调用方拿它决定渲染哪个表单，空值等于什么都不渲染。
 */
export function inferModelPlatform(config?: { modelPlatform?: string; baseUrl?: string }): ModelTypeList {
  if (isKnownModelPlatform(config?.modelPlatform)) {
    return config.modelPlatform
  }
  const baseUrl = (config?.baseUrl ?? '').toLowerCase()
  if (baseUrl.includes('volces.com')) {
    return ModelTypeList.Doubao
  }
  if (baseUrl.includes('openai.com')) {
    return ModelTypeList.OpenAI
  }
  if (baseUrl) {
    return ModelTypeList.Custom
  }
  // 没配过：与表单初始值保持一致
  return ModelTypeList.Doubao
}
export interface OptionInfo {
  value: string
  label: string
}
export interface ModelInfo {
  icon: ReactNode
  key: string
  value: string
  option?: OptionInfo[]
}

export const ModelInfoList = [
  {
    icon: <img src={doubao} className="!max-w-none w-[24px] h-[24px]" />,
    key: 'Doubao',
    value: 'doubao',
    option: [
      {
        value: 'doubao-seed-1-6-flash-250828',
        label: 'doubao-seed-1.6-flash'
      },
      {
        value: 'doubao-1-5-vision-pro-250328',
        label: 'doubao-1.5-vision-pro'
      },
      {
        value: 'doubao-1-5-vision-lite-250315',
        label: 'doubao-1.5-vision-lite'
      }
    ]
  },
  {
    icon: <img src={openAI} className="!max-w-none w-[24px] h-[24px]" />,
    key: 'OpenAI',
    value: 'openai',
    option: [
      {
        value: 'gpt-5',
        label: 'GPT-5'
      },
      {
        value: 'gpt-5-mini',
        label: 'GPT-5 Mini'
      },
      {
        value: 'gpt-5-nano',
        label: 'GPT-5 Nano'
      }
    ]
  },
  {
    icon: <img src={custom} className="!max-w-none w-[18px] h-[18px]" />,
    key: 'Custom',
    value: 'custom'
  }
]
