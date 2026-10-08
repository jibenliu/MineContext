// 渲染层日志：前缀、落盘出口的时机与容错。用 node --test 跑。

import assert from 'node:assert/strict'
import { test } from 'node:test'

import { formatLogArgs, getLogger, type LogLevel, setLogSink } from '../renderer.ts'

type ConsoleCall = { level: string; args: unknown[] }

function captureConsole(): { calls: ConsoleCall[]; restore: () => void } {
  const original = {
    error: console.error,
    warn: console.warn,
    info: console.info,
    debug: console.debug
  }
  const calls: ConsoleCall[] = []
  for (const level of ['error', 'warn', 'info', 'debug'] as const) {
    console[level] = (...args: unknown[]) => {
      calls.push({ level, args })
    }
  }
  return {
    calls,
    restore: () => {
      console.error = original.error
      console.warn = original.warn
      console.info = original.info
      console.debug = original.debug
    }
  }
}

test('日志带 scope 前缀并写进对应的 console 方法', () => {
  const capture = captureConsole()
  try {
    getLogger('Home').info('loaded', 1)

    assert.deepEqual(capture.calls, [{ level: 'info', args: ['[renderer - Home]', 'loaded', 1] }])
  } finally {
    capture.restore()
  }
})

test('无 scope 时用 [renderer] 前缀', () => {
  const capture = captureConsole()
  try {
    getLogger().warn('careful')

    assert.deepEqual(capture.calls, [{ level: 'warn', args: ['[renderer]', 'careful'] }])
  } finally {
    capture.restore()
  }
})

test('落盘出口在后装也能收到日志（业务代码在模块顶层就 getLogger）', () => {
  const capture = captureConsole()
  const written: { level: LogLevel; message: string }[] = []
  try {
    // 先取日志器，再装出口 —— 顺序反过来是很常见的真实情况
    const logger = getLogger('Store')
    setLogSink((level, message) => written.push({ level, message }))

    logger.error('boom', { code: 7 })

    assert.deepEqual(written, [{ level: 'error', message: '[renderer - Store] boom {"code":7}' }])
  } finally {
    setLogSink(undefined)
    capture.restore()
  }
})

test('出口抛异常不会传到调用点（日志不该让业务挂掉）', () => {
  const capture = captureConsole()
  try {
    setLogSink(() => {
      throw new Error('外壳没起来')
    })

    assert.doesNotThrow(() => getLogger('Boot').info('still runs'))
    assert.equal(capture.calls.length, 1, '控制台那一路仍然要写出去')
  } finally {
    setLogSink(undefined)
    capture.restore()
  }
})

test('卸掉出口后不再转发', () => {
  const capture = captureConsole()
  const written: string[] = []
  try {
    setLogSink((_level, message) => written.push(message))
    getLogger('A').info('one')
    setLogSink(undefined)
    getLogger('A').info('two')

    assert.deepEqual(written, ['[renderer - A] one'])
  } finally {
    capture.restore()
  }
})

test('消息格式化：Error 取名字与信息，对象转 JSON', () => {
  assert.equal(formatLogArgs([new Error('炸了')]), 'Error: 炸了')
  assert.equal(formatLogArgs(['状态', { running: true }]), '状态 {"running":true}')
  assert.equal(formatLogArgs([undefined, null, 3]), 'undefined null 3')
})

test('循环引用不抛异常（退化成 String）', () => {
  const cyclic: Record<string, unknown> = { name: 'cyclic' }
  cyclic.self = cyclic

  assert.doesNotThrow(() => formatLogArgs([cyclic]))
  assert.match(formatLogArgs([cyclic]), /cyclic/)
})

test('超长消息被截断（日志文件不该被一个对象刷爆）', () => {
  const long = 'x'.repeat(5000)
  const formatted = formatLogArgs([long])

  assert.ok(formatted.length < 5000, '必须截断')
  assert.match(formatted, /已截断/)
})
