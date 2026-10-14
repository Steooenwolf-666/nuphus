import { describe, expect, it } from 'vitest'
import { composeAssistantReplies } from '../core/progressMessages'
import type { ChatMessage } from '../core/types'

const records: ChatMessage[] = [
  { id: 'user', role: 'user', content: '完成临时文件任务。', timestamp: 1 },
  {
    id: 'p1',
    reply_id: 'draft',
    message_id: 'p1',
    role: 'assistant',
    kind: 'progress',
    content: '先检查文件。',
    timestamp: 2,
  },
  {
    id: 'p2',
    reply_id: 'draft',
    message_id: 'p2',
    role: 'assistant',
    kind: 'progress',
    content: '正在验证结果。',
    timestamp: 3,
  },
  { id: 'draft', role: 'assistant', content: '任务已完成。', timestamp: 4, runtime: 'done' },
]

describe('compose with cleared progress (post-completion shape)', () => {
  it('cleared progress (completed 收敛后) renders only the final reply', () => {
    const cleared = records.map(m => (m.kind === 'progress' ? { ...m, content: '' } : m))
    const visible = composeAssistantReplies(cleared)
    // user 气泡原样保留；progress 清空后不再与 final 拼接，只剩最终回复
    expect(visible.map(m => m.content)).toEqual(['完成临时文件任务。', '任务已完成。'])
  })
})
