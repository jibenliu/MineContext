// 对话里的「总结某段时间」：把一句话解析成范围。
//
// 只认**确定性**的少量写法（今天/昨天/前天/最近 N 天/最近一周/日期到日期）。
// 识别不出就返回 null，由调用方明确回一句「没听懂时段」—— 绝不猜一个范围
// 去生成：用户要的是本地库里的真实总结，猜错范围比不生成更糟。

import dayjs from 'dayjs'

export interface SummaryIntent {
  from: string
  to: string
  label: string
}

const DAY = 'YYYY-MM-DD'

function range(from: dayjs.Dayjs, to: dayjs.Dayjs, label: string): SummaryIntent {
  return { from: from.format(DAY), to: to.format(DAY), label }
}

export function parseSummaryIntent(text: string, now: dayjs.Dayjs = dayjs()): SummaryIntent | null {
  const value = text.trim()
  if (!value) return null

  // 触发词：没有它就不当指令，避免把「今天天气怎么样」这类闲聊截走
  if (!/总结|汇总|小结/.test(value)) return null

  if (/今天|今日/.test(value)) return range(now, now, '今天')
  if (/昨天|昨日/.test(value)) {
    const day = now.subtract(1, 'day')
    return range(day, day, '昨天')
  }
  if (/前天/.test(value)) {
    const day = now.subtract(2, 'day')
    return range(day, day, '前天')
  }

  const recent = value.match(/最近\s*(\d+)\s*[天日]/)
  if (recent) {
    const days = Number(recent[1])
    if (!Number.isInteger(days) || days <= 0) return null
    return range(now.subtract(days - 1, 'day'), now, `最近 ${days} 天`)
  }
  if (/最近一周|这周|本周/.test(value)) return range(now.subtract(6, 'day'), now, '最近一周')

  const explicit = value.match(/(\d{4}-\d{2}-\d{2})\s*(?:到|至|~|-|—)\s*(\d{4}-\d{2}-\d{2})/)
  if (explicit) {
    const left = dayjs(explicit[1])
    const right = dayjs(explicit[2])
    if (!left.isValid() || !right.isValid()) return null
    // 反着说也应生效：按时间先后摆正，而不是报错
    const [from, to] = left.isAfter(right) ? [right, left] : [left, right]
    const fromDay = from.format(DAY)
    const toDay = to.format(DAY)
    return { from: fromDay, to: toDay, label: `${fromDay} 至 ${toDay}` }
  }

  return null
}
