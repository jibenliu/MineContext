// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import { Button, Form, Input, Message, Select, Spin, Typography } from '@arco-design/web-react'
import { useI18n } from '@renderer/i18n'
import { getLogger } from '@shared/logger/renderer'
import { useMemoizedFn, useMount, useRequest } from 'ahooks'
import { find, get, isEmpty, pick } from 'lodash'
import { FC, useEffect, useMemo, useRef, useState } from 'react'

import { getModelInfo, getStoredApiKey, ModelConfigProps, updateModelSettingsAPI } from '../../services/settings'
import { LanguageSwitch } from './components/language-switch'
import { LaunchAtLoginSwitch } from './components/launch-at-login-switch'
import ModelRadio from './components/model-radio/model-radio'
import { NotificationSwitch } from './components/notification-switch'
import {
  BaseUrl,
  embeddingModels,
  inferModelPlatform,
  isKnownModelPlatform,
  ModelInfoList,
  ModelTypeList
} from './constants'

const logger = getLogger('settings')

const FormItem = Form.Item
const { Text } = Typography

interface SettingsProps {
  closeSetting?: () => void
  init?: boolean
}
export interface InputPrefixProps {
  label: string
}
const InputPrefix: FC<InputPrefixProps> = (props) => {
  const { label } = props
  return <div className="flex w-[73px] items-center">{label}</div>
}

/** 密钥输入：脱敏回显 + 复制；已配置时允许留空/保持脱敏串以沿用旧密钥。 */
const ApiKeyField: FC<{
  field: string
  className?: string
  autoFocus?: boolean
  hasStoredKey: boolean
  maskedValue: string
  onCopy: () => void
  docsLabel: string
  onOpenDocs: () => void
}> = ({ field, className, autoFocus, hasStoredKey, maskedValue, onCopy, docsLabel, onOpenDocs }) => {
  const { t } = useI18n()
  return (
    <FormItem
      requiredSymbol={false}
      label={t('common.apiKey')}
      field={field}
      extra={
        <div className="flex flex-col gap-1 text-[var(--color-text-3)] text-[14px]">
          {hasStoredKey ? <span>{t('settings.apiKeyConfiguredHint')}</span> : null}
          <div className="flex flex-wrap items-center gap-1">
            {t('settings.apiKeyGetHint')}
            <Button type="text" onClick={onOpenDocs} className="!px-1">
              {docsLabel}
            </Button>
            {hasStoredKey ? (
              <Button type="text" onClick={onCopy} className="!px-1">
                {t('settings.apiKeyCopy')}
              </Button>
            ) : null}
          </div>
        </div>
      }
      rules={[
        {
          validator(value, callback) {
            const text = typeof value === 'string' ? value.trim() : ''
            if (text || hasStoredKey) {
              callback()
              return
            }
            callback(t('settings.required'))
          }
        }
      ]}>
      <Input.Password
        autoFocus={autoFocus}
        placeholder={hasStoredKey && maskedValue ? maskedValue : t('settings.apiKeyPlaceholder')}
        allowClear
        className={className ?? '!w-[574px]'}
        defaultVisibility={false}
      />
    </FormItem>
  )
}

export interface CustomFormItemsProps {
  prefix: string
  hasStoredKey: boolean
  maskedValue: string
  onCopyApiKey: () => void
}
const CustomFormItems: FC<CustomFormItemsProps> = (props) => {
  const { t } = useI18n()
  const { prefix, hasStoredKey, maskedValue, onCopyApiKey } = props
  return (
    <>
      <div className="flex flex-col gap-6 mb-6">
        <div className="flex flex-col gap-[8px]">
          <span className="text-[var(--color-text-1)] font-roboto text-base font-normal leading-[22px] ">
            {t('settings.visionModel')}
          </span>
          <FormItem
            field={`${prefix}-modelId`}
            className="!mb-0"
            rules={[{ required: true, message: t('settings.required') }]}
            requiredSymbol={false}>
            <Input
              addBefore={<InputPrefix label={t('settings.modelName')} />}
              placeholder={t('settings.modelIdPlaceholder')}
              allowClear
              className="[&_.arco-input-inner-wrapper]: !w-[574px]"
            />
          </FormItem>
          <FormItem
            field={`${prefix}-baseUrl`}
            className="!mb-0"
            rules={[{ required: true, message: t('settings.required') }]}
            requiredSymbol={false}>
            <Input
              addBefore={<InputPrefix label={t('settings.baseUrl')} />}
              placeholder={t('settings.baseUrlPlaceholder')}
              allowClear
              className="[&_.arco-input-inner-wrapper]: !w-[574px]"
            />
          </FormItem>
          <FormItem
            field={`${prefix}-apiKey`}
            className="!mb-0"
            rules={[
              {
                validator(value, callback) {
                  const text = typeof value === 'string' ? value.trim() : ''
                  if (text || hasStoredKey) {
                    callback()
                    return
                  }
                  callback(t('settings.required'))
                }
              }
            ]}
            requiredSymbol={false}
            extra={
              hasStoredKey ? (
                <div className="flex items-center gap-2 text-[var(--color-text-3)] text-[13px]">
                  <span>{t('settings.apiKeyConfiguredHint')}</span>
                  <Button type="text" size="mini" onClick={onCopyApiKey}>
                    {t('settings.apiKeyCopy')}
                  </Button>
                </div>
              ) : null
            }>
            <Input.Password
              addBefore={<InputPrefix label={t('common.apiKey')} />}
              placeholder={hasStoredKey && maskedValue ? maskedValue : t('settings.apiKeyPlaceholder')}
              allowClear
              className="!w-[574px]"
              defaultVisibility={false}
            />
          </FormItem>
        </div>
        <div className="flex flex-col gap-[8px]">
          <span className="text-[var(--color-text-1)] font-roboto text-base font-normal leading-[22px]">
            {t('settings.embeddingModel')}
          </span>
          <FormItem
            field={`${prefix}-embeddingModelId`}
            className="!mb-0"
            rules={[{ required: true, message: t('settings.required') }]}
            requiredSymbol={false}>
            <Input
              addBefore={<InputPrefix label={t('settings.modelName')} />}
              placeholder={t('settings.embeddingModelPlaceholder')}
              allowClear
              className="!w-[574px]"
            />
          </FormItem>
          <FormItem
            field={`${prefix}-embeddingBaseUrl`}
            className="!mb-0"
            rules={[{ required: true, message: t('settings.required') }]}
            requiredSymbol={false}>
            <Input
              addBefore={<InputPrefix label={t('settings.baseUrl')} />}
              placeholder={t('settings.baseUrlPlaceholder')}
              allowClear
              className="!w-[574px]"
            />
          </FormItem>
          <FormItem
            field={`${prefix}-embeddingApiKey`}
            className="!mb-0"
            rules={[
              {
                validator(value, callback) {
                  const text = typeof value === 'string' ? value.trim() : ''
                  if (text || hasStoredKey) {
                    callback()
                    return
                  }
                  callback(t('settings.required'))
                }
              }
            ]}
            requiredSymbol={false}>
            <Input.Password
              addBefore={<InputPrefix label={t('common.apiKey')} />}
              placeholder={hasStoredKey && maskedValue ? maskedValue : t('settings.apiKeyPlaceholder')}
              allowClear
              className="!w-[574px]"
              defaultVisibility={false}
            />
          </FormItem>
        </div>
      </div>
    </>
  )
}
export interface StandardFormItemsProps {
  modelPlatform: ModelTypeList
  prefix: string
  hasStoredKey: boolean
  maskedValue: string
  onCopyApiKey: () => void
}
const StandardFormItems: FC<StandardFormItemsProps> = (props) => {
  const { t } = useI18n()
  const { modelPlatform, prefix, hasStoredKey, maskedValue, onCopyApiKey } = props
  const option = useMemo(() => {
    const foundItem = find(ModelInfoList, (item) => item.value === modelPlatform)
    return foundItem ? foundItem.option : []
  }, [modelPlatform])

  return (
    <>
      <FormItem
        label={t('settings.selectAiModel')}
        field={`${prefix}-modelId`}
        requiredSymbol={false}
        rules={[
          {
            validator(value, callback) {
              if (!value) {
                callback('Please select AI model')
              } else {
                callback()
              }
            }
          }
        ]}>
        <Select allowCreate placeholder={t('settings.selectPlaceholder')} options={option} className="!w-[574px]" />
      </FormItem>
      <ApiKeyField
        field={`${prefix}-apiKey`}
        autoFocus
        hasStoredKey={hasStoredKey}
        maskedValue={maskedValue}
        onCopy={onCopyApiKey}
        docsLabel={
          modelPlatform === ModelTypeList.Doubao ? t('settings.getDoubaoApiKey') : t('settings.getOpenaiApiKey')
        }
        onOpenDocs={() => {
          const url =
            modelPlatform === ModelTypeList.Doubao
              ? 'https://www.volcengine.com/docs/82379/1541594'
              : 'https://platform.openai.com/settings/organization/api-keys'
          window.open(url)
        }}
      />
    </>
  )
}

export interface SettingsFormBase {
  modelPlatform: string
}

export type SettingsFormProps = SettingsFormBase & {
  [K in ModelTypeList as `${K}-modelId` | `${K}-apiKey`]?: string
} & {
  [K in
    | `${ModelTypeList.Custom}-embeddingModelId`
    | `${ModelTypeList.Custom}-embeddingBaseUrl`
    | `${ModelTypeList.Custom}-embeddingApiKey`]?: string
}
const Settings: FC<SettingsProps> = (props) => {
  const { t } = useI18n()
  const { closeSetting, init } = props

  const [form] = Form.useForm<SettingsFormProps>()
  const { run: getInfo, loading: getInfoLoading, data: modelInfo } = useRequest(getModelInfo, { manual: true })
  const [hasStoredKey, setHasStoredKey] = useState(false)
  const maskedRef = useRef('')

  const { run: updateModelSettings, loading: updateLoading } = useRequest(updateModelSettingsAPI, {
    manual: true,
    onSuccess() {
      Message.success(t('settings.saved'))
      getInfo()
      if (init) {
        closeSetting?.()
      }
    },
    onError(e: Error) {
      const errMsg = get(e, 'response.data.message') || get(e, 'message') || t('settings.saveFailed')
      Message.error(errMsg)
    }
  })

  const copyStoredApiKey = useMemoizedFn(async () => {
    try {
      const key = await getStoredApiKey()
      if (!key) {
        Message.error(t('settings.apiKeyCopyFailed'))
        return
      }
      await navigator.clipboard.writeText(key)
      Message.success(t('settings.apiKeyCopied'))
    } catch (error) {
      logger.error('[settings] 复制 API Key 失败', error)
      Message.error(t('settings.apiKeyCopyFailed'))
    }
  })

  const submit = useMemoizedFn(async () => {
    try {
      await form.validate()
      const values = form.getFieldsValue()
      const isCustom = values.modelPlatform === ModelTypeList.Custom
      if (!values.modelPlatform) {
        Message.error(t('settings.selectPlatform'))
        return
      }
      const apiKeyField = `${values.modelPlatform}-apiKey` as keyof SettingsFormProps
      const rawKey = String(values[apiKeyField] ?? '').trim()
      // 脱敏串或空串 = 未改密钥，交给后端沿用已有引用
      const effectiveKey = !rawKey || rawKey === maskedRef.current ? '' : rawKey
      if (!effectiveKey && !hasStoredKey) {
        Message.error(t('settings.required'))
        return
      }

      const commonKey = [
        'modelPlatform',
        `${values.modelPlatform}-modelId`,
        `${values.modelPlatform}-apiKey`,
        `${values.modelPlatform}-baseUrl`,
        `${values.modelPlatform}-embeddingModelId`,
        `${values.modelPlatform}-embeddingBaseUrl`,
        `${values.modelPlatform}-embeddingApiKey`
      ]
      const data = pick(values, commonKey)
      data[`${values.modelPlatform}-apiKey`] = effectiveKey
      if (isCustom) {
        const embField = `${ModelTypeList.Custom}-embeddingApiKey` as keyof SettingsFormProps
        const embRaw = String(values[embField] ?? '').trim()
        data[`${ModelTypeList.Custom}-embeddingApiKey`] = !embRaw || embRaw === maskedRef.current ? '' : embRaw
      }
      const formatData = Object.fromEntries(
        Object.entries(data).map(([key, value]) => [key.replace(`${values.modelPlatform}-`, ''), value])
      )
      const params = isCustom
        ? formatData
        : {
            ...formatData,
            baseUrl: values.modelPlatform === ModelTypeList.Doubao ? BaseUrl.DoubaoUrl : BaseUrl.OpenAIUrl,
            embeddingModelId:
              values.modelPlatform === ModelTypeList.Doubao
                ? embeddingModels.DoubaoEmbeddingModelId
                : embeddingModels.OpenAIEmbeddingModelId
          }

      updateModelSettings(params as unknown as ModelConfigProps)
    } catch (error) {
      // 保存模型配置失败已经有全局提示（axios 拦截器），这里不重复弹窗
      logger.error('[settings] 保存模型配置失败', error)
    }
  })

  useMount(() => {
    getInfo()
  })
  // 后端不带平台名（见 `inferModelPlatform`），所以平台选择由配置反推：
  // 表单渲染与回填都以它为准，避免出现「平台字段是空串」这种没有对应表单的状态。
  const backendPlatform = useMemo(
    () => inferModelPlatform(get(modelInfo, 'config') as ModelConfigProps | undefined),
    [modelInfo]
  )

  useEffect(() => {
    const config = get(modelInfo, 'config')
    const hasKey = Boolean(get(modelInfo, 'hasApiKey'))
    const masked = String(get(modelInfo, 'apiKeyMasked') || '')
    setHasStoredKey(hasKey)
    maskedRef.current = masked
    if (!getInfoLoading && !isEmpty(config) && !init) {
      const settingsValue = new Map<keyof SettingsFormProps, string>()
      const prefix = backendPlatform
      settingsValue.set(`modelPlatform`, prefix)
      Object.keys(config).reduce((acc, key) => {
        if (!acc.has(`${prefix}-${key}` as keyof SettingsFormProps) && !!config[key]) {
          acc.set(`${prefix}-${key}` as keyof SettingsFormProps, config[key])
        }
        return acc
      }, settingsValue)
      // get 的 apiKey 恒为空；用脱敏串回填，用户能看见「已有密钥」
      if (hasKey && masked) {
        settingsValue.set(`${prefix}-apiKey` as keyof SettingsFormProps, masked)
        if (prefix === ModelTypeList.Custom) {
          settingsValue.set(`${prefix}-embeddingApiKey` as keyof SettingsFormProps, masked)
        }
      }
      form.setFieldsValue(Object.fromEntries(settingsValue))
    }
  }, [modelInfo, getInfoLoading, form, init, backendPlatform])

  return (
    <Spin loading={getInfoLoading} block className="[&_.arco-spin-children]:!h-full !h-full">
      <div className="top-0 left-0 flex flex-col h-full overflow-y-hidden py-2 pr-2 relative">
        <div className="bg-[var(--color-bg-2)] rounded-[16px] pl-6 flex flex-col h-full overflow-y-auto overflow-x-hidden scrollbar-hide pb-2">
          <div className="mb-[12px]">
            <div className="mt-[26px] mb-[10px] text-[24px] font-bold text-[var(--color-text-1)]">
              {t('settings.heading')}
            </div>
            <Text type="secondary" className="text-[13px]">
              {t('settings.subheading')}
            </Text>
          </div>

          <div>
            <Form
              autoComplete="off"
              layout={'vertical'}
              form={form}
              initialValues={{
                modelPlatform: ModelTypeList.Doubao,
                [`${ModelTypeList.Doubao}-modelId`]: 'doubao-seed-1-6-flash-250828',
                [`${ModelTypeList.OpenAI}-modelId`]: 'gpt-5-nano'
              }}>
              <FormItem label={t('settings.modelPlatform')} field={'modelPlatform'} requiredSymbol={false}>
                <ModelRadio />
              </FormItem>
              <FormItem
                shouldUpdate={(prevValues, currentValues) => prevValues.modelPlatform !== currentValues.modelPlatform}
                noStyle>
                {(values) => {
                  // 认不出平台名时退回配置反推的结果：三个分支都不匹配就等于整片表单消失，
                  // 那时页面上只剩一个保存按钮，用户无从下手。
                  const selected = values.modelPlatform
                  const modelPlatform = isKnownModelPlatform(selected) ? selected : backendPlatform
                  if (modelPlatform === ModelTypeList.Custom) {
                    return (
                      <CustomFormItems
                        prefix={ModelTypeList.Custom}
                        hasStoredKey={hasStoredKey}
                        maskedValue={maskedRef.current}
                        onCopyApiKey={copyStoredApiKey}
                      />
                    )
                  }
                  if (modelPlatform === ModelTypeList.OpenAI) {
                    return (
                      <StandardFormItems
                        modelPlatform={modelPlatform}
                        prefix={ModelTypeList.OpenAI}
                        hasStoredKey={hasStoredKey}
                        maskedValue={maskedRef.current}
                        onCopyApiKey={copyStoredApiKey}
                      />
                    )
                  }
                  return (
                    <StandardFormItems
                      modelPlatform={ModelTypeList.Doubao}
                      prefix={ModelTypeList.Doubao}
                      hasStoredKey={hasStoredKey}
                      maskedValue={maskedRef.current}
                      onCopyApiKey={copyStoredApiKey}
                    />
                  )
                }}
              </FormItem>
            </Form>
            <Spin loading={updateLoading}>
              <Button
                type="primary"
                onClick={submit}
                disabled={updateLoading}
                className="!bg-[rgb(var(--primary-6))] !border-[rgb(var(--primary-6))]">
                {init ? t('settings.getStarted') : t('settings.save')}
              </Button>
            </Spin>
          </div>

          {/* 通用设置：与上面的模型配置不是一回事，所以单独分组，
              避免把「开机自启」混进 API key 的表单字段流里。
              用一行「标题 + 说明 + 控件」而不是大边框卡片：一个开关撑满整行会很空。 */}
          <div className="mt-[20px] border-t border-[var(--color-border-2)] pt-[16px]">
            <div className="mb-[8px] text-[14px] font-bold text-[var(--color-text-1)]">{t('settings.startup')}</div>
            <LaunchAtLoginSwitch />
            <NotificationSwitch />
            {/* 语言切换入口：设置页里的位置固定在启动项下面，不随页面结构漂移 */}
            <LanguageSwitch />
          </div>
        </div>
      </div>
    </Spin>
  )
}

export default Settings
