// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import { Button, Form, Input, Message, Select, Typography } from '@arco-design/web-react'
import { useI18n } from '@renderer/i18n'
import { getLogger } from '@shared/logger/renderer'
import { useMemoizedFn, useMount, useRequest } from 'ahooks'
import { find, get, isEmpty, pick } from 'lodash'
import { FC, useEffect, useMemo, useRef, useState } from 'react'

import { ErrorBoundary } from '../../components/error-boundary'
import FirstRunChecklist from '../../components/first-run-checklist'
import { useFirstRunChecklist } from '../../components/first-run-checklist/use-first-run-checklist'
import {
  getModelInfo,
  getStoredApiKey,
  isPlainApiKeyCandidate,
  ModelConfigProps,
  ModelInfoResponseData,
  ProviderSettingsMask,
  updateModelSettingsAPI
} from '../../services/settings'
import { writeClipboard } from '../../utils/write-clipboard'
import { apiKeyInputShouldBeVisible } from './api-key-visibility'
import { AiUploadSwitch } from './components/ai-upload-switch'
import { BackfillSection } from './components/backfill-section'
import { LanguageSwitch } from './components/language-switch'
import { LaunchAtLoginSwitch } from './components/launch-at-login-switch'
import ModelRadio from './components/model-radio/model-radio'
import { NotificationSwitch } from './components/notification-switch'
import { UpdateCheckSection } from './components/update-check-section'
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
  /** 供 Form.useWatch（宽类型，避免与 SettingsFormProps 循环引用）。 */
  form: any
  onCopy: () => void
  /** 眼睛打开时把脱敏串换成明文，避免「可见但仍是 ••••」 */
  onReveal: () => void
  docsLabel: string
  onOpenDocs: () => void
  label?: string
}> = ({
  field,
  className,
  autoFocus,
  hasStoredKey,
  maskedValue,
  form,
  onCopy,
  onReveal,
  docsLabel,
  onOpenDocs,
  label
}) => {
  const { t } = useI18n()
  const [userWantsVisible, setUserWantsVisible] = useState(false)
  const fieldValue = String(Form.useWatch(field, form) ?? '')
  const visible = apiKeyInputShouldBeVisible({
    hasStoredKey,
    maskedValue,
    fieldValue,
    userWantsVisible
  })
  return (
    <FormItem
      requiredSymbol={false}
      label={label ?? t('common.apiKey')}
      field={field}
      extra={
        <div className="flex flex-col gap-1 text-[var(--color-text-3)] text-[14px]">
          {hasStoredKey ? (
            <span data-testid="api-key-configured-hint">{t('settings.apiKeyConfiguredHint')}</span>
          ) : null}
          <div className="flex flex-wrap items-center gap-1">
            {t('settings.apiKeyGetHint')}
            <Button type="text" onClick={onOpenDocs} className="!px-1">
              {docsLabel}
            </Button>
            <Button type="text" onClick={onCopy} className="!px-1">
              {t('settings.apiKeyCopy')}
            </Button>
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
        data-testid={`api-key-input-${field}`}
        visibility={visible}
        onVisibilityChange={(next) => {
          const showingMask =
            hasStoredKey &&
            Boolean(maskedValue) &&
            (!fieldValue.trim() || fieldValue.trim() === maskedValue || fieldValue.includes('•'))
          // 脱敏已按明文可见：再点眼睛是「换成真实明文」，不要盖成圆点假空白
          if (showingMask) {
            setUserWantsVisible(true)
            onReveal()
            return
          }
          setUserWantsVisible(next)
          if (next) onReveal()
        }}
      />
    </FormItem>
  )
}

/** 自建表单里带 addBefore 的密钥框：与 ApiKeyField 同一套「脱敏必须可见」契约。 */
const PrefixedApiKeyPassword: FC<{
  field: string
  form: any
  hasStoredKey: boolean
  maskedValue: string
  onReveal: () => void
  /** Form.Item 注入；必须落到 Input，否则回填脱敏串进不了受控值。 */
  value?: string
  onChange?: (value: string) => void
}> = ({ field, form, hasStoredKey, maskedValue, onReveal, value, onChange }) => {
  const { t } = useI18n()
  const [userWantsVisible, setUserWantsVisible] = useState(false)
  const watched = String(Form.useWatch(field, form) ?? '')
  const fieldValue = String(value ?? watched)
  const visible = apiKeyInputShouldBeVisible({
    hasStoredKey,
    maskedValue,
    fieldValue,
    userWantsVisible
  })
  return (
    <Input.Password
      value={value}
      onChange={onChange}
      addBefore={<InputPrefix label={t('common.apiKey')} />}
      placeholder={hasStoredKey && maskedValue ? maskedValue : t('settings.apiKeyPlaceholder')}
      allowClear
      className="!w-[574px]"
      data-testid={`api-key-input-${field}`}
      visibility={visible}
      onVisibilityChange={(next) => {
        const showingMask =
          hasStoredKey &&
          Boolean(maskedValue) &&
          (!fieldValue.trim() || fieldValue.trim() === maskedValue || fieldValue.includes('•'))
        if (showingMask) {
          setUserWantsVisible(true)
          onReveal()
          return
        }
        setUserWantsVisible(next)
        if (next) onReveal()
      }}
    />
  )
}

export interface CustomFormItemsProps {
  prefix: string
  hasStoredKey: boolean
  maskedValue: string
  hasStoredEmbeddingKey: boolean
  embeddingMaskedValue: string
  form: any
  onCopyApiKey: (field: keyof SettingsFormProps) => void
  onRevealApiKey: (field: keyof SettingsFormProps) => void
}
const CustomFormItems: FC<CustomFormItemsProps> = (props) => {
  const { t } = useI18n()
  const {
    prefix,
    hasStoredKey,
    maskedValue,
    hasStoredEmbeddingKey,
    embeddingMaskedValue,
    form,
    onCopyApiKey,
    onRevealApiKey
  } = props
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
              <div className="flex items-center gap-2 text-[var(--color-text-3)] text-[13px]">
                {hasStoredKey ? (
                  <span data-testid="api-key-configured-hint">{t('settings.apiKeyConfiguredHint')}</span>
                ) : null}
                <Button
                  type="text"
                  size="mini"
                  onClick={() => onCopyApiKey(`${prefix}-apiKey` as keyof SettingsFormProps)}>
                  {t('settings.apiKeyCopy')}
                </Button>
              </div>
            }>
            <PrefixedApiKeyPassword
              field={`${prefix}-apiKey`}
              form={form}
              hasStoredKey={hasStoredKey}
              maskedValue={maskedValue}
              onReveal={() => onRevealApiKey(`${prefix}-apiKey` as keyof SettingsFormProps)}
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
                  if (text || hasStoredEmbeddingKey) {
                    callback()
                    return
                  }
                  callback(t('settings.required'))
                }
              }
            ]}
            requiredSymbol={false}
            extra={
              <div className="flex items-center gap-2 text-[var(--color-text-3)] text-[13px]">
                {hasStoredEmbeddingKey ? (
                  <span data-testid="api-key-configured-hint">{t('settings.apiKeyConfiguredHint')}</span>
                ) : null}
                <Button
                  type="text"
                  size="mini"
                  onClick={() => onCopyApiKey(`${prefix}-embeddingApiKey` as keyof SettingsFormProps)}>
                  {t('settings.apiKeyCopy')}
                </Button>
              </div>
            }>
            <PrefixedApiKeyPassword
              field={`${prefix}-embeddingApiKey`}
              form={form}
              hasStoredKey={hasStoredEmbeddingKey}
              maskedValue={embeddingMaskedValue}
              onReveal={() => onRevealApiKey(`${prefix}-embeddingApiKey` as keyof SettingsFormProps)}
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
  form: any
  onCopyApiKey: (field: keyof SettingsFormProps) => void
  onRevealApiKey: (field: keyof SettingsFormProps) => void
}
const StandardFormItems: FC<StandardFormItemsProps> = (props) => {
  const { t } = useI18n()
  const { modelPlatform, prefix, hasStoredKey, maskedValue, form, onCopyApiKey, onRevealApiKey } = props
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
        form={form}
        onCopy={() => onCopyApiKey(`${prefix}-apiKey` as keyof SettingsFormProps)}
        onReveal={() => onRevealApiKey(`${prefix}-apiKey` as keyof SettingsFormProps)}
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
/** HashRouter 下 query 在 hash 里：`#/settings?section=ai-upload` */
function settingsSectionFromLocation(): string | null {
  if (typeof window === 'undefined') return null
  const hashQuery = window.location.hash.includes('?') ? window.location.hash.split('?')[1] : ''
  const search = hashQuery || window.location.search.replace(/^\?/, '')
  return new URLSearchParams(search).get('section')
}

const Settings: FC<SettingsProps> = (props) => {
  const { t } = useI18n()
  const { closeSetting, init } = props

  const [form] = Form.useForm<SettingsFormProps>()
  const { run: getInfo, loading: getInfoLoading, data: modelInfo } = useRequest(getModelInfo, { manual: true })
  const [hasStoredKey, setHasStoredKey] = useState(false)
  const [maskedValue, setMaskedValue] = useState('')
  const maskedRef = useRef('')
  const [hasStoredEmbeddingKey, setHasStoredEmbeddingKey] = useState(false)
  const [embeddingMaskedValue, setEmbeddingMaskedValue] = useState('')
  const embeddingMaskedRef = useRef('')
  /** 各平台脱敏与 hasKey；切换 Radio 时据此更新当前框，避免误用活跃平台的状态。 */
  const providersRef = useRef<Record<string, ProviderSettingsMask>>({})

  const applyProviderKeyState = useMemoizedFn((platform: string, providers: Record<string, ProviderSettingsMask>) => {
    const slot = providers[platform]
    const hasKey = Boolean(slot?.hasApiKey)
    const masked = String(slot?.apiKeyMasked || '')
    const hasEmb = Boolean(slot?.hasEmbeddingApiKey)
    const embMasked = String(slot?.embeddingApiKeyMasked || '')
    setHasStoredKey(hasKey)
    setMaskedValue(masked)
    maskedRef.current = masked
    setHasStoredEmbeddingKey(hasEmb)
    setEmbeddingMaskedValue(embMasked)
    embeddingMaskedRef.current = embMasked
  })

  const firstRun = useFirstRunChecklist({ apiKeyConfiguredOverride: hasStoredKey })

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

  const storedFieldForFormField = (field: string): 'vision' | 'embedding' =>
    field.includes('embeddingApiKey') ? 'embedding' : 'vision'

  const providerForFormField = (field: string): string => {
    for (const p of [ModelTypeList.Doubao, ModelTypeList.OpenAI, ModelTypeList.Custom]) {
      if (field.startsWith(`${p}-`)) {
        return p
      }
    }
    return String(form.getFieldValue('modelPlatform') || '')
  }

  const maskForFormField = (field: string): string => {
    const provider = providerForFormField(field)
    const slot = providersRef.current[provider]
    if (storedFieldForFormField(field) === 'embedding') {
      return String(slot?.embeddingApiKeyMasked || embeddingMaskedRef.current || '')
    }
    return String(slot?.apiKeyMasked || maskedRef.current || '')
  }

  const copyStoredApiKey = useMemoizedFn(async (field?: keyof SettingsFormProps) => {
    try {
      // Doubao / OpenAI / Custom 视觉与向量密钥字段都走这里；
      // 刚输入、尚未被 get 回填成脱敏串时，优先用表单里的明文。
      const platform = String(form.getFieldValue('modelPlatform') || '')
      const target = field ?? (`${platform}-apiKey` as keyof SettingsFormProps)
      const fromForm = String(form.getFieldValue(target) ?? '')
      const mask = maskForFormField(String(target))
      const provider = providerForFormField(String(target))
      let key = ''
      if (isPlainApiKeyCandidate(fromForm, mask)) {
        key = fromForm.trim()
      } else {
        key = await getStoredApiKey(storedFieldForFormField(String(target)), provider)
      }
      if (!key) {
        Message.error(t('settings.apiKeyCopyFailed'))
        return
      }
      await writeClipboard(key)
      Message.success(t('settings.apiKeyCopied'))
    } catch (error) {
      logger.error('[settings] 复制 API Key 失败', error)
      Message.error(t('settings.apiKeyClipboardFailed'))
    }
  })

  /** 眼睛打开：脱敏回显换成明文，便于核对与手动复制。 */
  const revealStoredApiKey = useMemoizedFn(async (field: keyof SettingsFormProps) => {
    const fromForm = String(form.getFieldValue(field) ?? '')
    const mask = maskForFormField(String(field))
    if (isPlainApiKeyCandidate(fromForm, mask)) {
      return
    }
    try {
      const key = await getStoredApiKey(storedFieldForFormField(String(field)), providerForFormField(String(field)))
      if (!key) {
        return
      }
      form.setFieldValue(field, key)
    } catch (error) {
      logger.error('[settings] 显示 API Key 明文失败', error)
    }
  })

  const submit = useMemoizedFn(async () => {
    // 每次点击都先落盘：校验失败若只弹 Toast 不写日志，Toast 被挡时无法从 renderer.log 排查。
    logger.info('[settings] 点击保存/开始使用', { init: Boolean(init), hasStoredKey })
    try {
      await form.validate()
      const values = form.getFieldsValue()
      const isCustom = values.modelPlatform === ModelTypeList.Custom
      if (!values.modelPlatform) {
        logger.warn('[settings] 未选择模型平台')
        Message.error(t('settings.selectPlatform'))
        return
      }
      const apiKeyField = `${values.modelPlatform}-apiKey` as keyof SettingsFormProps
      const rawKey = String(values[apiKeyField] ?? '').trim()
      const platformMask = String(providersRef.current[values.modelPlatform]?.apiKeyMasked || maskedRef.current)
      const platformHasKey = Boolean(providersRef.current[values.modelPlatform]?.hasApiKey ?? hasStoredKey)
      // 脱敏串或空串 = 未改密钥，交给后端沿用该平台已存分档
      const effectiveKey = !rawKey || rawKey === platformMask ? '' : rawKey
      if (!effectiveKey && !platformHasKey) {
        logger.warn('[settings] API Key 为空且无已存密钥，拒绝提交')
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
        const customSlot = providersRef.current[ModelTypeList.Custom]
        const embMask = String(customSlot?.embeddingApiKeyMasked || embeddingMaskedRef.current)
        // 脱敏串（视觉或向量）都表示「未改」；向量槽与视觉槽独立落盘
        const embUnchanged = !embRaw || embRaw === embMask || embRaw === platformMask
        data[`${ModelTypeList.Custom}-embeddingApiKey`] = embUnchanged ? '' : embRaw
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
      logger.error('[settings] 保存模型配置失败', error)
      // 表单校验失败时 axios 拦截器不会跑：必须在这里给可见反馈，否则像死按钮。
      // Arco validate 拒绝值常为字段错误对象，不能直接当 message 展示。
      const errMsg = get(error, 'response.data.message') || get(error, 'message')
      Message.error(typeof errMsg === 'string' && errMsg ? errMsg : t('settings.saveFailed'))
    }
  })

  const skipOnboarding = useMemoizedFn(() => {
    logger.info('[settings] 用户跳过引导，进入主界面')
    closeSetting?.()
  })

  useMount(() => {
    getInfo()
    const section = settingsSectionFromLocation()
    if (section) {
      requestAnimationFrame(() => {
        document.getElementById(section)?.scrollIntoView({ behavior: 'smooth', block: 'start' })
      })
    }
  })

  // 后端现在会回 `modelPlatform`（活跃平台）；旧响应仍可能是空串，那时按 base_url 反推。
  const backendPlatform = useMemo(
    () => inferModelPlatform(get(modelInfo, 'config') as ModelConfigProps | undefined),
    [modelInfo]
  )

  useEffect(() => {
    const info = modelInfo as ModelInfoResponseData | undefined
    const config = get(info, 'config')
    const providers = { ...(get(info, 'providers') || {}) } as Record<string, ProviderSettingsMask>
    // 活跃平台顶层字段并入 providers，兼容尚未写 providers 的旧响应
    const active = backendPlatform
    if (!providers[active] && (get(info, 'hasApiKey') || get(info, 'apiKeyMasked'))) {
      providers[active] = {
        modelId: get(config, 'modelId'),
        baseUrl: get(config, 'baseUrl'),
        embeddingModelId: get(config, 'embeddingModelId'),
        embeddingBaseUrl: get(config, 'embeddingBaseUrl'),
        hasApiKey: Boolean(get(info, 'hasApiKey')),
        apiKeyMasked: String(get(info, 'apiKeyMasked') || ''),
        hasEmbeddingApiKey: Boolean(get(info, 'hasEmbeddingApiKey')),
        embeddingApiKeyMasked: String(get(info, 'embeddingApiKeyMasked') || '')
      }
    }
    providersRef.current = providers
    applyProviderKeyState(active, providers)

    // 引导态也要回填：SSE 丢帧时用户仍停在 init 页，若跳过回填则「开始使用」
    // 会用默认豆包字段覆盖已有 OpenAI/自建配置。无配置的新用户 config 为空，走 initialValues。
    if (!getInfoLoading && !isEmpty(config)) {
      const settingsValue = new Map<keyof SettingsFormProps, string>()
      settingsValue.set(`modelPlatform`, active)

      // 先回填各平台分档（含非活跃），再覆盖活跃平台的 config 字段
      for (const [platform, slot] of Object.entries(providers)) {
        if (!isKnownModelPlatform(platform)) continue
        if (slot.modelId) {
          settingsValue.set(`${platform}-modelId` as keyof SettingsFormProps, slot.modelId)
        }
        if (slot.baseUrl && platform === ModelTypeList.Custom) {
          settingsValue.set(`${platform}-baseUrl` as keyof SettingsFormProps, slot.baseUrl)
        }
        if (slot.embeddingModelId && platform === ModelTypeList.Custom) {
          settingsValue.set(`${platform}-embeddingModelId` as keyof SettingsFormProps, slot.embeddingModelId)
        }
        if (slot.embeddingBaseUrl && platform === ModelTypeList.Custom) {
          settingsValue.set(`${platform}-embeddingBaseUrl` as keyof SettingsFormProps, slot.embeddingBaseUrl)
        }
        if (slot.hasApiKey && slot.apiKeyMasked) {
          settingsValue.set(`${platform}-apiKey` as keyof SettingsFormProps, slot.apiKeyMasked)
        }
        if (platform === ModelTypeList.Custom) {
          const embFill =
            slot.hasEmbeddingApiKey && slot.embeddingApiKeyMasked
              ? slot.embeddingApiKeyMasked
              : slot.hasApiKey && slot.apiKeyMasked
                ? slot.apiKeyMasked
                : ''
          if (embFill) {
            settingsValue.set(`${platform}-embeddingApiKey` as keyof SettingsFormProps, embFill)
          }
        }
      }

      Object.keys(config).reduce((acc, key) => {
        if (key === 'modelPlatform' || key === 'apiKey' || key === 'embeddingApiKey') {
          return acc
        }
        if (!acc.has(`${active}-${key}` as keyof SettingsFormProps) && !!config[key]) {
          acc.set(`${active}-${key}` as keyof SettingsFormProps, config[key])
        }
        return acc
      }, settingsValue)

      form.setFieldsValue(Object.fromEntries(settingsValue))
    }
  }, [modelInfo, getInfoLoading, form, backendPlatform, applyProviderKeyState])

  // 不用整页 Spin 遮罩：getModelInfo 挂起时 Arco mask 会吞掉「开始使用」点击，
  // 且 onClick 进不来 → renderer.log 无任何新行，表现为死按钮。
  return (
    <div className="top-0 left-0 flex flex-col h-full overflow-y-hidden py-2 pr-2 relative">
      <div className="bg-[var(--color-bg-2)] rounded-[16px] pl-6 flex flex-col h-full overflow-y-auto overflow-x-hidden scrollbar-hide pb-2">
        <div className="mb-[12px]">
          <div className="mt-[26px] mb-[10px] text-[24px] font-bold text-[var(--color-text-1)]">
            {t('settings.heading')}
          </div>
          <Text type="secondary" className="text-[13px]">
            {t('settings.subheading')}
          </Text>
          {getInfoLoading ? (
            <Text type="secondary" className="mt-2 block text-[13px]" data-testid="settings-loading-hint">
              {t('common.loading')}
            </Text>
          ) : null}
        </div>

        {init && firstRun.visible ? (
          <FirstRunChecklist
            steps={firstRun.steps}
            apiKeyInline
            onRequestPermission={() => void firstRun.requestPermission()}
            onGoApiKey={() => undefined}
            onStartRecording={() => closeSetting?.()}
            onClearWaiting={firstRun.clearWaiting}
            onDismiss={firstRun.dismiss}
          />
        ) : null}

        <div>
          <Form
            autoComplete="off"
            layout={'vertical'}
            form={form}
            initialValues={{
              modelPlatform: ModelTypeList.Doubao,
              [`${ModelTypeList.Doubao}-modelId`]: 'doubao-seed-1-6-flash-250828',
              [`${ModelTypeList.OpenAI}-modelId`]: 'gpt-5-nano'
            }}
            onValuesChange={(changed) => {
              // 一点切换平台：立刻换当前平台的「已配置 / 脱敏」状态（表单字段已按平台分前缀）
              if (changed.modelPlatform && isKnownModelPlatform(changed.modelPlatform)) {
                applyProviderKeyState(changed.modelPlatform, providersRef.current)
              }
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
                      maskedValue={maskedValue}
                      hasStoredEmbeddingKey={hasStoredEmbeddingKey}
                      embeddingMaskedValue={embeddingMaskedValue}
                      form={form}
                      onCopyApiKey={copyStoredApiKey}
                      onRevealApiKey={revealStoredApiKey}
                    />
                  )
                }
                if (modelPlatform === ModelTypeList.OpenAI) {
                  return (
                    <StandardFormItems
                      modelPlatform={modelPlatform}
                      prefix={ModelTypeList.OpenAI}
                      hasStoredKey={hasStoredKey}
                      maskedValue={maskedValue}
                      form={form}
                      onCopyApiKey={copyStoredApiKey}
                      onRevealApiKey={revealStoredApiKey}
                    />
                  )
                }
                return (
                  <StandardFormItems
                    modelPlatform={ModelTypeList.Doubao}
                    prefix={ModelTypeList.Doubao}
                    hasStoredKey={hasStoredKey}
                    maskedValue={maskedValue}
                    form={form}
                    onCopyApiKey={copyStoredApiKey}
                    onRevealApiKey={revealStoredApiKey}
                  />
                )
              }}
            </FormItem>
          </Form>
          <div className="flex flex-wrap items-center gap-3">
            <Button
              type="primary"
              data-testid="settings-submit"
              onClick={submit}
              loading={updateLoading}
              disabled={updateLoading}
              className="!bg-[rgb(var(--primary-6))] !border-[rgb(var(--primary-6))]">
              {init ? t('settings.getStarted') : t('settings.save')}
            </Button>
            {init ? (
              <Button type="text" data-testid="settings-skip-onboarding" onClick={skipOnboarding}>
                {t('settings.skipForNow')}
              </Button>
            ) : null}
          </div>
        </div>

        {/* 通用设置：与上面的模型配置不是一回事，所以单独分组，
              避免把「开机自启」混进 API key 的表单字段流里。
              用一行「标题 + 说明 + 控件」而不是大边框卡片：一个开关撑满整行会很空。 */}
        <div id="model" className="scroll-mt-6" />
        <div className="mt-[20px] border-t border-[var(--color-border-2)] pt-[16px]">
          <div className="mb-[8px] text-[14px] font-bold text-[var(--color-text-1)]">{t('settings.privacy')}</div>
          {/* 引导页也会挂载本块：局部降级，避免隐私开关把整页设置打成白屏 */}
          <ErrorBoundary title={t('settings.privacy')}>
            <AiUploadSwitch />
          </ErrorBoundary>
        </div>
        <div className="mt-[20px] border-t border-[var(--color-border-2)] pt-[16px]">
          <div className="mb-[8px] text-[14px] font-bold text-[var(--color-text-1)]">{t('settings.startup')}</div>
          <LaunchAtLoginSwitch />
          <NotificationSwitch />
          <UpdateCheckSection />
          {/* 语言切换入口：设置页里的位置固定在启动项下面，不随页面结构漂移 */}
          <LanguageSwitch />
          <BackfillSection />
        </div>
      </div>
    </div>
  )
}

export default Settings
