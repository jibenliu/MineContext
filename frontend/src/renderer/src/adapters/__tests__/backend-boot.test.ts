import assert from 'node:assert/strict'
import { test } from 'node:test'

import { loadingStatusForBootPhase, shouldOfferSettingsEscape, shouldShowBackendUnavailable } from '../backend-boot.ts'

test('成功启动（ready）不展示无法连接本地服务', () => {
  assert.equal(shouldShowBackendUnavailable('ready'), false)
  assert.equal(loadingStatusForBootPhase('ready'), 'running')
})

test('等待外壳/daemon 写 runtime（waiting）用 starting，不是硬错误文案', () => {
  assert.equal(shouldShowBackendUnavailable('waiting'), false)
  assert.equal(loadingStatusForBootPhase('waiting'), 'starting')
})

test('确认失败（failed）才升为 error，才允许无法连接文案', () => {
  assert.equal(shouldShowBackendUnavailable('failed'), true)
  assert.equal(loadingStatusForBootPhase('failed'), 'error')
})

// 硬错误文案已写「可从设置继续配置」，UI 必须给出对应出口，不能只剩重试死循环。
test('failed 态必须提供进入设置的出口', () => {
  assert.equal(shouldOfferSettingsEscape('failed'), true)
})

test('waiting / ready 不抢设置出口（避免冷启动误导）', () => {
  assert.equal(shouldOfferSettingsEscape('waiting'), false)
  assert.equal(shouldOfferSettingsEscape('ready'), false)
})
