/**
 * ChatStreamReducer 契约测试（C8）：SSE 协议事件序列 → store 消息结构。
 *
 * C1 深模块（lib/chat-stream.ts）的直接测试面——协议归约逻辑不再只能
 * 经 UI（ChatPanel.test）间接验证。
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { createChatStreamReducer } from '@/lib/chat-stream';
import { useAppStore } from '@/lib/store';
import type { ChatStreamEvent } from '@/lib/types';
import { messageText } from '@/lib/types';

function ev(partial: Partial<ChatStreamEvent>): ChatStreamEvent {
  return {
    id: 'e1',
    session_id: 's1',
    delta: '',
    chunk_type: 'answer',
    ...partial,
  } as ChatStreamEvent;
}

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState());
});

describe('createChatStreamReducer', () => {
  it('establishes the streaming session on the first chunk', () => {
    const r = createChatStreamReducer();
    r.handleEvent(ev({ session_id: 's1', delta: '你好' }));
    expect(r.streamSessionId).toBe('s1');
    expect(useAppStore.getState().currentSessionId).toBe('s1');
  });

  it('accumulates thinking, deltas and tool calls into the assistant message', () => {
    useAppStore.getState().addMessage({ role: 'assistant', segments: [], id: null, timestamp: '' });
    const r = createChatStreamReducer();
    r.handleEvent(ev({ thinking: '先想' }));
    r.handleEvent(ev({ delta: '正文' }));
    r.handleEvent(
      ev({ tool_call: { id: 't1', name: 'read_file', arguments: '{}', presentation: 'read' } }),
    );

    const assistant = useAppStore.getState().messages.filter((m) => m.role === 'assistant');
    expect(assistant).toHaveLength(1);
    expect(assistant[0].thinking).toBe('先想');
    expect(messageText(assistant[0])).toBe('正文');
    expect(assistant[0].tool_calls?.[0]).toMatchObject({ id: 't1', name: 'read_file' });
  });

  it('applies tool results to the matching tool call', () => {
    useAppStore.getState().addMessage({ role: 'assistant', segments: [], id: null, timestamp: '' });
    const r = createChatStreamReducer();
    r.handleEvent(
      ev({ tool_call: { id: 't1', name: 'ls', arguments: '{}', presentation: 'terminal' } }),
    );
    r.handleEvent(
      ev({
        tool_result: { tool_call_id: 't1', duration_ms: 42, success: true, content: 'result-ok' },
      }),
    );
    const tool = useAppStore.getState().messages[0].tool_calls?.[0];
    expect(tool).toMatchObject({ duration_ms: 42, success: true, result: 'result-ok' });
  });

  it('handles error chunks: removes the empty bubble, resets status, shows toast', () => {
    useAppStore.getState().addMessage({ role: 'assistant', segments: [], id: null, timestamp: '' });
    useAppStore.getState().setStreamStatus('streaming');
    const r = createChatStreamReducer({ errorFallbackText: '处理失败' });
    r.handleEvent(ev({ chunk_type: 'error', delta: '请求校验失败' }));
    const state = useAppStore.getState();
    expect(state.messages.filter((m) => m.role === 'assistant')).toHaveLength(0);
    expect(Object.values(state.streamStatus).every((s) => s === 'idle')).toBe(true);
    expect(state.toasts[0]?.message).toBe('请求校验失败');
  });

  it('marks interrupted on finish_reason interrupted and toasts once', () => {
    useAppStore.getState().addMessage({ role: 'assistant', segments: [], id: null, timestamp: '' });
    const r = createChatStreamReducer();
    r.handleEvent(ev({ delta: '部分输出', finish_reason: 'interrupted' }));
    const assistant = useAppStore.getState().messages[0];
    expect(assistant.interrupted).toBe(true);
    expect(useAppStore.getState().toasts[0]?.message).toBe('流式中断，已保留部分输出');
  });

  it('marks truncation on finish_reason length and attaches usage', () => {
    useAppStore.getState().addMessage({ role: 'assistant', segments: [], id: null, timestamp: '' });
    const r = createChatStreamReducer();
    r.handleEvent(ev({ delta: '部分输出', finish_reason: 'length' }));
    r.handleEvent(
      ev({
        usage: {
          prompt_tokens: 100,
          completion_tokens: 50,
          total_tokens: 150,
          cache_read: 40,
          cache_write: 10,
          context_window: 16000,
        },
      }),
    );
    const assistant = useAppStore.getState().messages[0];
    expect(assistant.truncated_by_length).toBe(true);
    expect(assistant.usage?.total_tokens).toBe(150);
    expect(r.liveWindow).toBe(16000);
  });

  it('starts a new assistant turn when thinking arrives after content', () => {
    // 真实模式（ADR-028）：handleSend 只加 assistant 占位（id: null），
    // user 消息由服务端边界事件提供
    useAppStore.getState().addMessage({ role: 'assistant', segments: [], id: null, timestamp: '' });
    const r = createChatStreamReducer();
    r.handleEvent(ev({ thinking: '第一轮思考' }));
    r.handleEvent(ev({ delta: '第一轮输出' }));
    r.handleEvent(ev({ thinking: '第二轮思考' }));
    const assistants = useAppStore.getState().messages.filter((m) => m.role === 'assistant');
    expect(assistants).toHaveLength(2);
    expect(assistants[1].thinking).toBe('第二轮思考');
  });

  it('confirms the optimistic user message id via user_message_id event (ADR-031)', () => {
    // 乐观渲染：本地插入用户消息（带 user_message_id）→ 确认事件比对后
    // 替换为服务端真实 id（user_message_id 字段删除——生命周期结束）。
    // 新会话场景：确认事件是首个事件（adoptOnFirstEvent 迁移 PENDING → sid）
    const st = useAppStore.getState();
    st.addMessage({
      role: 'user',
      segments: [{ type: 'text', text: '你好' }],
      user_message_id: 'umid-1',
      id: null,
      timestamp: '',
    });
    st.addMessage({ role: 'assistant', segments: [], id: null, timestamp: '' });
    const r = createChatStreamReducer({ adoptOnFirstEvent: true });
    r.handleEvent(
      ev({ chunk_type: 'user_message_id', user_message_id: 'umid-1', message_id: 'msg_42' }),
    );
    const msgs = useAppStore.getState().messages;
    const user = msgs.find((m) => m.role === 'user');
    expect(user?.id).toBe('msg_42');
    expect(user?.user_message_id).toBeUndefined();
    expect(messageText(user!)).toBe('你好');
  });

  it('ignores user_message_id confirmation for unknown ids', () => {
    const st = useAppStore.getState();
    st.addMessage({
      role: 'user',
      segments: [{ type: 'text', text: '你好' }],
      user_message_id: 'umid-1',
      id: null,
      timestamp: '',
    });
    const r = createChatStreamReducer({ adoptOnFirstEvent: true });
    r.handleEvent(
      ev({ chunk_type: 'user_message_id', user_message_id: 'umid-unknown', message_id: 'msg_9' }),
    );
    const user = useAppStore.getState().messages.find((m) => m.role === 'user');
    // 未知 user_message_id：不匹配，乐观消息保持占位（id 仍为 null）
    expect(user?.id).toBeNull();
    expect(user?.user_message_id).toBe('umid-1');
  });
});


