// 启动握手的引导页判据：`llm` 是对象，不是布尔值。用 node --test 跑。

import assert from 'node:assert/strict'
import { test } from 'node:test'

import { shouldShowOnboarding } from '../onboarding.ts'

const payload = (llmStatus: string) =>
  JSON.stringify({ status: 'ok', data: { components: { llm: { status: llmStatus, message: '' } } } })

test('未配置视觉模型 → 进引导页', () => {
  assert.equal(shouldShowOnboarding(payload('unconfigured')), true)
})

test('已配置 → 直接进主界面', () => {
  assert.equal(shouldShowOnboarding(payload('ok')), false)
})

test('对象负载（已解析过的）同样适用', () => {
  assert.equal(shouldShowOnboarding({ data: { components: { llm: { status: 'ok' } } } }), false)
})

test('坏数据不抛异常，按「需要引导」处理', () => {
  assert.equal(shouldShowOnboarding('{ 不是 JSON'), true)
  assert.equal(shouldShowOnboarding(''), true)
  assert.equal(shouldShowOnboarding(undefined), true)
  assert.equal(shouldShowOnboarding(JSON.stringify({ status: 'ok' })), true)
})
