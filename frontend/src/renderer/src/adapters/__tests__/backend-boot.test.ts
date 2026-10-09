import assert from 'node:assert/strict'
import { test } from 'node:test'

import { loadingStatusForBootPhase, shouldShowBackendUnavailable } from '../backend-boot.ts'

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
