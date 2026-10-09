// 设置页 API Key 输入：已存密钥用脱敏串回填时必须按「可见文本」渲染。
//
// `Input.Password` 默认 type=password 会把脱敏串再盖成圆点；暗色主题下圆点与
// 背景接近，看起来像空白 —— 用户会以为没存过密钥、也无法辨认脱敏内容。

export function apiKeyInputShouldBeVisible(args: {
  hasStoredKey: boolean
  maskedValue: string
  fieldValue: string
  userWantsVisible: boolean
}): boolean {
  const masked = args.maskedValue.trim()
  const text = args.fieldValue.trim()
  if (args.hasStoredKey && masked && (!text || text === masked || text.includes('•'))) {
    return true
  }
  return args.userWantsVisible
}
