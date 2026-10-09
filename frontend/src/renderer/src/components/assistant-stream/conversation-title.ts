// 会话列表标题：库里允许 title 为空（建会话时未提问），界面不能退化成裸数字 id。

export function conversationDisplayTitle(
  conversation: { id: number; title?: string | null },
  untitled: string
): string {
  const title = typeof conversation.title === 'string' ? conversation.title.trim() : ''
  return title || untitled
}
