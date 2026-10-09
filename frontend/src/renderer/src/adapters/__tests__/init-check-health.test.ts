import assert from 'node:assert/strict'
import { test } from 'node:test'

import { fetchInitCheckFromHealth } from '../init-check-health.ts'
import { shouldShowOnboarding } from '../onboarding.ts'

test('health 负载可驱动引导判据（已配置 → 不进引导）', async () => {
  const payload = {
    status: 'ok',
    data: { components: { llm: { status: 'ok', message: '' } } }
  }
  const raw = await fetchInitCheckFromHealth({
    getRuntime: async () => ({ port: 19790 }),
    fetch: async (url) => {
      assert.equal(String(url), 'http://127.0.0.1:19790/api/health')
      return {
        ok: true,
        status: 200,
        json: async () => payload
      } as Response
    }
  })
  assert.equal(shouldShowOnboarding(raw), false)
})

test('health 负载未配置模型 → 仍进引导', async () => {
  const payload = {
    status: 'ok',
    data: { components: { llm: { status: 'unconfigured', message: '未配置视觉模型' } } }
  }
  const raw = await fetchInitCheckFromHealth({
    getRuntime: async () => ({ port: 19790 }),
    fetch: async () =>
      ({
        ok: true,
        status: 200,
        json: async () => payload
      }) as Response
  })
  assert.equal(shouldShowOnboarding(raw), true)
})

test('health HTTP 非 2xx 时失败（不得静默当成已配置）', async () => {
  await assert.rejects(
    () =>
      fetchInitCheckFromHealth({
        getRuntime: async () => ({ port: 19790 }),
        fetch: async () =>
          ({
            ok: false,
            status: 503,
            json: async () => ({})
          }) as Response
      }),
    /503/
  )
})

test('无 runtime 端口时失败（调用方不得当成「已配置」）', async () => {
  await assert.rejects(
    () =>
      fetchInitCheckFromHealth({
        getRuntime: async () => null,
        fetch: async () => {
          throw new Error('不应发起请求')
        }
      }),
    /端口/
  )
})
