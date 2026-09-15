/**
 * 会话流式事件纯函数测试（C8）：SSE 协议事件序列 → store 消息结构。
 *
 * C1 深模块（lib/chat-stream.ts）的直接测试面——`handleChatStreamEvent`
 * 为订阅级常驻纯函数（无实例/注册表），事件序列直接驱动 store 映射。
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { handleChatStreamEvent } from '@/lib/chat-stream';
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
  // 纯函数不改变当前会话（事件 → sessionMessages[sid] 字典，不 adopt）
  useAppStore.getState().setCurrentSession('s1');
});

describe('handleChatStreamEvent', () => {
  it('accumulates thinking, deltas and tool calls into the assistant message', () => {
    useAppStore.getState().addMessage({ role: 'assistant', segments: [], id: null, timestamp: '' });
    handleChatStreamEvent(ev({ thinking: '先想' }));
    handleChatStreamEvent(ev({ delta: '正文' }));
    handleChatStreamEvent(
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
    handleChatStreamEvent(
      ev({ tool_call: { id: 't1', name: 'ls', arguments: '{}', presentation: 'terminal' } }),
    );
    handleChatStreamEvent(
      ev({
        tool_result: { tool_call_id: 't1', duration_ms: 42, success: true, content: 'result-ok' },
      }),
    );
    const tool = useAppStore.getState().messages[0].tool_calls?.[0];
    expect(tool).toMatchObject({ duration_ms: 42, success: true, result: 'result-ok' });
  });

  it('ask_user 事件把追问绑定到来源会话（跨会话不串）', () => {
    handleChatStreamEvent(
      ev({
        session_id: 'session-A',
        tool_call: {
          id: 't-ask',
          name: 'ask_user',
          arguments: JSON.stringify({ questions: [{ question: '确认吗？', options: [] }] }),
          presentation: 'generic',
        },
      }),
    );

    const pending = useAppStore.getState().pendingClarification;
    expect(pending?.questions[0]?.question).toBe('确认吗？');
    // 修复前无 sessionId：追问是全局状态 → 在其他会话也会弹（并在该会话提交 → 原会话卡死）
    expect(pending?.sessionId).toBe('session-A');
  });

  it('handles error chunks: removes the empty bubble, resets status, shows toast', () => {
    useAppStore.getState().addMessage({ role: 'assistant', segments: [], id: null, timestamp: '' });
    useAppStore.getState().setStreamStatus('streaming');
    handleChatStreamEvent(ev({ chunk_type: 'error', delta: '请求校验失败' }));
    const state = useAppStore.getState();
    expect(state.messages.filter((m) => m.role === 'assistant')).toHaveLength(0);
    expect(Object.values(state.streamStatus).every((s) => s === 'idle')).toBe(true);
    expect(state.toasts[0]?.message).toBe('请求校验失败');
  });

  it('marks interrupted on finish_reason interrupted and toasts once', () => {
    useAppStore.getState().addMessage({ role: 'assistant', segments: [], id: null, timestamp: '' });
    handleChatStreamEvent(ev({ delta: '部分输出', finish_reason: 'interrupted' }));
    const assistant = useAppStore.getState().messages[0];
    expect(assistant.interrupted).toBe(true);
    expect(useAppStore.getState().toasts[0]?.message).toBe('流式中断，已保留部分输出');
  });

  it('marks truncation on finish_reason length and attaches usage', () => {
    useAppStore.getState().addMessage({ role: 'assistant', segments: [], id: null, timestamp: '' });
    handleChatStreamEvent(ev({ delta: '部分输出', finish_reason: 'length' }));
    handleChatStreamEvent(
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
  });

  it('starts a new assistant turn when thinking arrives after content', () => {
    // 真实模式（ADR-028）：handleSend 只加 assistant 占位（id: null），
    // user 消息由服务端边界事件提供
    useAppStore.getState().addMessage({ role: 'assistant', segments: [], id: null, timestamp: '' });
    handleChatStreamEvent(ev({ thinking: '第一轮思考' }));
    handleChatStreamEvent(ev({ delta: '第一轮输出' }));
    handleChatStreamEvent(ev({ thinking: '第二轮思考' }));
    const assistants = useAppStore.getState().messages.filter((m) => m.role === 'assistant');
    expect(assistants).toHaveLength(2);
    expect(assistants[1].thinking).toBe('第二轮思考');
  });

  it('starts a new assistant turn when thinking arrives after a system notification', () => {
    // 后台任务完成通知（system 消息）落库推送后，唤醒轮思考到达——最后
    // 一条是 system 时必须新开 assistant 消息，否则 thinking 落到通知
    // 之前的 assistant 上（"思考插到通知前面"根因回归保护）
    useAppStore.getState().addMessage({ role: 'assistant', segments: [], id: null, timestamp: '' });
    handleChatStreamEvent(ev({ delta: '等任务完成后我会收到注入通知' }));
    // system 通知边界事件（applyServerMessage 按 id 追加）
    handleChatStreamEvent(
      ev({
        message: {
          id: 'msg-sys-1',
          role: 'system',
          segments: [{ type: 'text', text: '[后台命令完成] test（cmd_0）' }],
        },
      }),
    );
    // 唤醒轮思考到达
    handleChatStreamEvent(ev({ thinking: '后台命令已完成，汇总结果' }));

    const msgs = useAppStore.getState().messages;
    const assistants = msgs.filter((m) => m.role === 'assistant');
    // 第一条 assistant（启动任务时的回复）+ 新开的唤醒轮 assistant
    expect(assistants).toHaveLength(2);
    // 新开的 assistant 在 system 通知之后（思考不插到通知前面）
    const sysIdx = msgs.findIndex((m) => m.id === 'msg-sys-1');
    const wakeIdx = msgs.findIndex(
      (m) => m.role === 'assistant' && m.thinking === '后台命令已完成，汇总结果',
    );
    expect(wakeIdx).toBeGreaterThan(sysIdx);
  });

  it('appends the compression summary message via server boundary event (pushed like normal messages)', () => {
    // 压缩摘要是会话时序链上的普通节点（统一结构，不做区分）：与 System
    // 通知同一条推送路径（chat_stream 边界事件 → applyServerMessage 按 id
    // 查重追加）——压缩点在页面上实时可见，无需重载会话；usage 随消息
    // 携带（圆环 lastMessageUsage 据此估算压缩后占用）。压缩点为 user 锚定
    //（模型侧角色），但按 marker 走独立消息路径（非用户输入，不做乐观合并）。
    useAppStore.getState().addMessage({ role: 'assistant', segments: [], id: null, timestamp: '' });
    handleChatStreamEvent(ev({ delta: '回复完成，压缩检查在后台进行' }));
    handleChatStreamEvent(
      ev({
        message: {
          id: 'cmp_1',
          role: 'user',
          compression_marker: true,
          segments: [{ type: 'text', text: '[对话摘要] 以下是对历史对话的摘要：……' }],
          usage: {
            prompt_tokens: 167284,
            completion_tokens: 3542,
            total_tokens: 170826,
            cache_read: 0,
            cache_write: 0,
          },
        },
      }),
    );

    const msgs = useAppStore.getState().messages;
    // 摘要消息按 id 追加（时序顺序：排在回复之后）
    const marker = msgs.find((m) => m.id === 'cmp_1');
    expect(marker?.compression_marker).toBe(true);
    expect(msgs[msgs.length - 1].id).toBe('cmp_1');
    // usage 透传（lastMessageUsage 的压缩点分支依赖）
    expect(marker?.usage?.prompt_tokens).toBe(167284);
  });

  it('appends the compression marker even when an optimistic user message is pending', () => {
    // 回归保护：压缩点是 user 锚定的独立节点（非用户输入）——乐观合并的
    // "存在未确认 user_message_id 则跳过"逻辑不适用于它；该场景下压缩点
    // 必须仍按 id 幂等追加（修复前会被静默跳过、压缩点丢失）。
    useAppStore.getState().addMessage({
      role: 'user',
      segments: [{ type: 'text', text: '进行中的输入' }],
      user_message_id: 'umid-pending',
      id: null,
      timestamp: '',
    });
    handleChatStreamEvent(
      ev({
        message: {
          id: 'cmp_pending_1',
          role: 'user',
          compression_marker: true,
          segments: [{ type: 'text', text: '[对话摘要] ……' }],
        },
      }),
    );

    const msgs = useAppStore.getState().messages;
    // 压缩点按 id 追加成功（未被乐观消息误判跳过）
    expect(msgs.some((m) => m.id === 'cmp_pending_1')).toBe(true);
    // 乐观消息仍保留（未被误动）
    expect(msgs.some((m) => m.user_message_id === 'umid-pending')).toBe(true);
  });

  it('confirms the optimistic user message id via user_message_id event (ADR-031)', () => {
    // 乐观渲染：本地插入用户消息（带 user_message_id）→ 确认事件比对后
    // 替换为服务端真实 id（user_message_id 字段删除——生命周期结束）
    const st = useAppStore.getState();
    st.addMessage({
      role: 'user',
      segments: [{ type: 'text', text: '你好' }],
      user_message_id: 'umid-1',
      id: null,
      timestamp: '',
    });
    st.addMessage({ role: 'assistant', segments: [], id: null, timestamp: '' });
    handleChatStreamEvent(
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
    handleChatStreamEvent(
      ev({ chunk_type: 'user_message_id', user_message_id: 'umid-unknown', message_id: 'msg_9' }),
    );
    const user = useAppStore.getState().messages.find((m) => m.role === 'user');
    // 未知 user_message_id：不匹配，乐观消息保持占位（id 仍为 null）
    expect(user?.id).toBeNull();
    expect(user?.user_message_id).toBe('umid-1');
  });

  it('applies message boundary events into the session store (wake loop included)', () => {
    // 唤醒轮/子代理事件：携带完整结构化消息 → applyServerMessage 写入
    // 指定会话字典（无归约器注册表——事件常驻可达）
    handleChatStreamEvent(
      ev({
        message: {
          role: 'assistant',
          id: 'msg_wake_1',
          segments: [{ type: 'text', text: '后台任务已完成：结果汇总如下' }],
          timestamp: '2026-09-08T00:00:00Z',
        },
      }),
    );
    const msgs = useAppStore.getState().sessionMessages['s1'];
    expect(msgs).toHaveLength(1);
    expect(msgs[0].id).toBe('msg_wake_1');
    expect(msgs[0].role).toBe('assistant');
  });

  it('resets stream status on normal completion without any lifecycle', () => {
    useAppStore.getState().setStreamStatus('streaming');
    handleChatStreamEvent(ev({ delta: '回答', finish_reason: 'stop' }));
    expect(useAppStore.getState().streamStatus['s1'] ?? 'idle').toBe('idle');
  });
});
