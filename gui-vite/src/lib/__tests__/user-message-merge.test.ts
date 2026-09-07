import { describe, it, expect, beforeEach } from 'vitest';
import { useAppStore } from '@/lib/store';
import type { ChatMessage } from '@/lib/types';

function userMsg(id: string, content: string): ChatMessage {
  return {
    id,
    role: 'user',
    segments: [{ type: 'text', text: content }],
    timestamp: '2026-09-05T00:00:00Z',
  };
}

function assistantPlaceholder(): ChatMessage {
  return { id: null, role: 'assistant', segments: [], timestamp: '2026-09-05T00:00:00Z' };
}

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState());
});

describe('用户消息边界事件（applyServerMessage，ADR-031）', () => {
  it('新会话：边界事件先到（PENDING 迁移后插入），不重复', () => {
    useAppStore.getState().addMessage(assistantPlaceholder());

    useAppStore.getState().setCurrentSession('session-1');
    useAppStore.getState().applyServerMessage('session-1', userMsg('msg_1', '你好'));
    expect(useAppStore.getState().messages.map((m) => m.id)).toEqual(['msg_1', null]);
  });

  it('乐观渲染后：本地已有 user_message_id 的乐观消息——边界事件跳过（确认事件负责 id 同步）', () => {
    // handleSend（ADR-031）：本地插入乐观 user 消息（user_message_id 定位）
    useAppStore.getState().addMessage({
      role: 'user',
      segments: [{ type: 'text', text: '你好' }],
      user_message_id: 'umid-1',
      id: null,
      timestamp: '',
    });
    useAppStore.getState().addMessage(assistantPlaceholder());

    // 边界事件（用户消息完整版）到达：乐观消息已存在 → 跳过，不重复插入
    useAppStore.getState().applyServerMessage('session-1', userMsg('msg_1', '你好'));
    const users = useAppStore.getState().messages.filter((m) => m.role === 'user');
    expect(users).toHaveLength(1);
    expect(users[0].user_message_id).toBe('umid-1');
  });

  it('已有历史会话：边界事件 user2 应插到末尾（assistant 占位前）', () => {
    useAppStore.setState({
      currentSessionId: 'session-1',
      sessionMessages: {
        'session-1': [
          userMsg('msg_1', '第一轮问题'),
          { id: 'msg_2', role: 'assistant', segments: [{ type: 'text', text: '第一轮回答' }], timestamp: '' },
        ],
      },
      messages: [
        userMsg('msg_1', '第一轮问题'),
        { id: 'msg_2', role: 'assistant', segments: [{ type: 'text', text: '第一轮回答' }], timestamp: '' },
      ],
    });

    useAppStore.getState().addMessage(assistantPlaceholder());

    // 边界事件 user2 到达
    useAppStore.getState().applyServerMessage('session-1', userMsg('msg_3', '第二轮问题'));

    const msgs = useAppStore.getState().messages;
    expect(msgs.map((m) => m.id)).toEqual(['msg_1', 'msg_2', 'msg_3', null]);
  });
});
