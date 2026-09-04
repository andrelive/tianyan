import { describe, it, expect, beforeEach } from 'vitest';
import { useAppStore, PENDING_SESSION_KEY } from '@/lib/store';
import type { ChatMessage, Session, SkillCallInfo } from '@/lib/types';

describe('useAppStore', () => {
  // Reset store to initial values before each test
  beforeEach(() => {
    useAppStore.setState({
      currentView: 'chat',
      currentSessionId: null,
      sessions: [],
      messages: [],
      sessionMessages: {},
      streamStatus: {},
      theme: 'system',
      fontSize: 'medium',
      apiBaseUrl: 'http://localhost:3000',
      toasts: [],
      selectedModel: null,
      configured: null,
    });
  });

  // ── Initial State ──

  it('has correct initial state', () => {
    const state = useAppStore.getState();
    expect(state.currentView).toBe('chat');
    expect(state.currentSessionId).toBeNull();
    expect(state.sessions).toEqual([]);
    expect(state.messages).toEqual([]);
    expect(state.sessionMessages).toEqual({});
    expect(state.streamStatus).toEqual({});
    expect(state.theme).toBe('system');
    expect(state.fontSize).toBe('medium');
    expect(state.apiBaseUrl).toBe('http://localhost:3000');
    expect(state.toasts).toEqual([]);
    expect(state.selectedModel).toBeNull();
    expect(state.configured).toBeNull();
  });

  // ── View ──

  it('setView updates currentView', () => {
    const { setView } = useAppStore.getState();

    setView('settings');
    expect(useAppStore.getState().currentView).toBe('settings');

    setView('skills');
    expect(useAppStore.getState().currentView).toBe('skills');

    setView('knowledge');
    expect(useAppStore.getState().currentView).toBe('knowledge');

    setView('chat');
    expect(useAppStore.getState().currentView).toBe('chat');
  });

  // ── Sessions ──

  it('setSessions replaces the sessions array', () => {
    const sessions: Session[] = [
      { id: 's1', title: 'One', created_at: '', updated_at: '', message_count: 0 },
    ];

    useAppStore.getState().setSessions(sessions);
    expect(useAppStore.getState().sessions).toHaveLength(1);
    expect(useAppStore.getState().sessions[0].id).toBe('s1');
  });

  it('addSession appends a session', () => {
    const s1: Session = { id: '1', title: 'S1', created_at: '', updated_at: '', message_count: 0 };
    const s2: Session = { id: '2', title: 'S2', created_at: '', updated_at: '', message_count: 0 };

    useAppStore.getState().addSession(s1);
    expect(useAppStore.getState().sessions).toHaveLength(1);

    useAppStore.getState().addSession(s2);
    expect(useAppStore.getState().sessions).toHaveLength(2);
    expect(useAppStore.getState().sessions[1].id).toBe('2');
  });

  it('removeSession removes a session by id', () => {
    const s1: Session = { id: '1', title: 'S1', created_at: '', updated_at: '', message_count: 0 };
    const s2: Session = { id: '2', title: 'S2', created_at: '', updated_at: '', message_count: 0 };

    useAppStore.setState({ sessions: [s1, s2] });
    useAppStore.getState().removeSession('1');

    expect(useAppStore.getState().sessions).toHaveLength(1);
    expect(useAppStore.getState().sessions[0].id).toBe('2');
  });

  it('removeSession clears currentSessionId and messages when current session removed', () => {
    const s: Session = {
      id: 'cur',
      title: 'Current',
      created_at: '',
      updated_at: '',
      message_count: 0,
    };
    useAppStore.setState({
      sessions: [s],
      currentSessionId: 'cur',
      messages: [{ role: 'user', content: 'hello' }],
    });

    useAppStore.getState().removeSession('cur');

    expect(useAppStore.getState().currentSessionId).toBeNull();
    expect(useAppStore.getState().messages).toEqual([]);
    expect(useAppStore.getState().sessions).toHaveLength(0);
  });

  it('removeSession does not clear messages when removing a non-current session', () => {
    const s1: Session = { id: '1', title: 'A', created_at: '', updated_at: '', message_count: 0 };
    const s2: Session = { id: '2', title: 'B', created_at: '', updated_at: '', message_count: 0 };
    useAppStore.setState({
      sessions: [s1, s2],
      currentSessionId: '2',
      messages: [{ role: 'assistant', content: 'keep' }],
    });

    useAppStore.getState().removeSession('1');
    expect(useAppStore.getState().sessions).toHaveLength(1);
    expect(useAppStore.getState().currentSessionId).toBe('2');
    expect(useAppStore.getState().messages).toHaveLength(1);
  });

  // ── Messages ──

  it('setMessages replaces the messages array', () => {
    const msgs: ChatMessage[] = [{ role: 'user', content: 'first' }];

    useAppStore.getState().setMessages(msgs);
    expect(useAppStore.getState().messages).toHaveLength(1);
    expect(useAppStore.getState().messages[0].content).toBe('first');
  });

  it('addMessage appends a message', () => {
    useAppStore.getState().addMessage({ role: 'user', content: 'q' });
    useAppStore.getState().addMessage({ role: 'assistant', content: 'a' });

    const msgs = useAppStore.getState().messages;
    expect(msgs).toHaveLength(2);
    expect(msgs[0].role).toBe('user');
    expect(msgs[1].role).toBe('assistant');
  });

  it('updateLastMessage appends delta to the last message content', () => {
    useAppStore.setState({
      messages: [
        { role: 'user', content: 'hi' },
        { role: 'assistant', content: 'Hel' },
      ],
    });

    useAppStore.getState().updateLastMessage('lo');
    expect(useAppStore.getState().messages[1].content).toBe('Hello');
  });

  it('updateLastMessage does nothing when messages is empty', () => {
    useAppStore.setState({ messages: [] });

    useAppStore.getState().updateLastMessage('delta');
    expect(useAppStore.getState().messages).toEqual([]);
  });

  it('startNewAssistantTurn opens a new message after a completed turn', () => {
    // 上一轮已有正文 → 新开消息（轮次边界）
    useAppStore.setState({
      messages: [
        { role: 'user', content: 'hi' },
        { role: 'assistant', content: '完成回答', tool_calls: [] },
      ],
    });
    useAppStore.getState().startNewAssistantTurn();
    const msgs = useAppStore.getState().messages;
    expect(msgs).toHaveLength(3);
    expect(msgs[2].role).toBe('assistant');
    expect(msgs[2].content).toBe('');
  });

  it('startNewAssistantTurn reuses an empty placeholder', () => {
    // 空占位（流式初始）→ 复用，不新开
    useAppStore.setState({
      messages: [
        { role: 'user', content: 'hi' },
        { role: 'assistant', content: '', tool_calls: [] },
      ],
    });
    useAppStore.getState().startNewAssistantTurn();
    expect(useAppStore.getState().messages).toHaveLength(2);
  });

  it('clearMessages empties the messages array', () => {
    useAppStore.setState({ messages: [{ role: 'user', content: 'x' }] });

    useAppStore.getState().clearMessages();
    expect(useAppStore.getState().messages).toEqual([]);
  });

  it('deleteMessagesFrom removes messages from index and resets stream status', () => {
    useAppStore.setState({
      messages: [
        { role: 'user', content: 'a' },
        { role: 'assistant', content: 'b' },
        { role: 'user', content: 'c' },
      ],
      streamStatus: { [PENDING_SESSION_KEY]: 'streaming' },
    });

    useAppStore.getState().deleteMessagesFrom(1);
    expect(useAppStore.getState().messages).toHaveLength(1);
    expect(useAppStore.getState().messages[0].content).toBe('a');
    expect(useAppStore.getState().streamStatus).toEqual({ [PENDING_SESSION_KEY]: 'idle' });
  });

  // ── Skill Calls ──

  it('appendSkillCalls sets skill_calls on the last assistant message', () => {
    useAppStore.setState({
      messages: [
        { role: 'user', content: 'go' },
        { role: 'assistant', content: 'searching', skill_calls: [] },
      ],
    });

    const calls: SkillCallInfo[] = [
      { skill_id: 's1', skill_name: 'finder', success: true, execution_time_ms: 42 },
    ];
    useAppStore.getState().appendSkillCalls(calls);

    expect(useAppStore.getState().messages[1].skill_calls).toEqual(calls);
  });

  it('appendSkillCalls does nothing when no assistant message exists', () => {
    useAppStore.setState({
      messages: [{ role: 'user', content: 'hi' }],
    });

    // Should not throw and messages remain unchanged
    useAppStore
      .getState()
      .appendSkillCalls([{ skill_id: 's1', skill_name: 't', success: true, execution_time_ms: 0 }]);
    expect(useAppStore.getState().messages).toHaveLength(1);
    expect(useAppStore.getState().messages[0].skill_calls).toBeUndefined();
  });

  // ── Tool Calls（A2 展示契约）──

  it('appendToolCalls accumulates tool call cards on the last assistant message', () => {
    useAppStore.setState({
      messages: [
        { role: 'user', content: 'go' },
        { role: 'assistant', content: 'working' },
      ],
    });

    const call = { name: 'read_file', arguments: '{"path":"a.txt"}', presentation: 'read' };
    useAppStore.getState().appendToolCalls([call]);
    expect(useAppStore.getState().messages[1].tool_calls).toEqual([call]);

    // 同一调用重复推送（流式重发）不重复累积
    useAppStore.getState().appendToolCalls([call]);
    expect(useAppStore.getState().messages[1].tool_calls).toHaveLength(1);

    // 不同调用追加
    const call2 = { name: 'execute_command', arguments: '{}', presentation: 'terminal' };
    useAppStore.getState().appendToolCalls([call2]);
    expect(useAppStore.getState().messages[1].tool_calls).toHaveLength(2);
    expect(useAppStore.getState().messages[1].tool_calls?.[1]).toEqual(call2);
  });

  it('appendToolCalls does nothing when no assistant message exists', () => {
    useAppStore.setState({
      messages: [{ role: 'user', content: 'hi' }],
    });
    useAppStore
      .getState()
      .appendToolCalls([{ name: 'glob', arguments: '{}', presentation: 'search' }]);
    expect(useAppStore.getState().messages).toHaveLength(1);
    expect(useAppStore.getState().messages[0].tool_calls).toBeUndefined();
  });

  it('applyToolResult matches tool call by id (流式结果挂卡)', () => {
    useAppStore.setState({
      messages: [
        { role: 'user', content: 'hi' },
        { role: 'assistant', content: '' },
      ],
    });
    // 流式 tool_call 事件带调用 ID
    useAppStore
      .getState()
      .appendToolCalls([
        { id: 'call_abc', name: 'read_file', arguments: '{}', presentation: 'read' },
      ]);
    // observation 结果事件按 tool_call_id 关联
    useAppStore.getState().applyToolResult({
      tool_call_id: 'call_abc',
      duration_ms: 120,
      success: true,
      content: '{"content":"file body"}',
    });
    const card = useAppStore.getState().messages[1].tool_calls?.[0];
    expect(card?.duration_ms).toBe(120);
    expect(card?.success).toBe(true);
    expect(card?.result).toBe('{"content":"file body"}');

    // 不匹配的 tool_call_id 不更新任何卡片
    useAppStore.getState().applyToolResult({
      tool_call_id: 'call_zzz',
      duration_ms: 5,
      success: false,
      error: 'boom',
    });
    expect(useAppStore.getState().messages[1].tool_calls?.[0]?.duration_ms).toBe(120);
  });

  // ── Streaming ──

  it('setStreamStatus updates streamStatus per session', () => {
    const { setStreamStatus } = useAppStore.getState();

    setStreamStatus('streaming');
    expect(useAppStore.getState().streamStatus).toEqual({ [PENDING_SESSION_KEY]: 'streaming' });

    setStreamStatus('error');
    expect(useAppStore.getState().streamStatus).toEqual({ [PENDING_SESSION_KEY]: 'error' });

    setStreamStatus('idle');
    expect(useAppStore.getState().streamStatus).toEqual({ [PENDING_SESSION_KEY]: 'idle' });
  });

  // ── Toast ──

  it('showToast appends toasts (stacking)', () => {
    useAppStore.getState().showToast('Something failed', 'error');
    useAppStore.getState().showToast('Done!', 'success');
    useAppStore.getState().showToast('FYI', 'info');
    const toasts = useAppStore.getState().toasts;
    expect(toasts).toHaveLength(3);
    expect(toasts[0].message).toBe('Something failed');
    expect(toasts[0].type).toBe('error');
    expect(toasts[1].message).toBe('Done!');
    expect(toasts[1].type).toBe('success');
    expect(toasts[2].message).toBe('FYI');
    expect(toasts[2].type).toBe('info');
    expect(toasts.every((t) => typeof t.id === 'string' && t.id.length > 0)).toBe(true);
  });

  it('hideToast removes the toast by id', () => {
    useAppStore.getState().showToast('hello', 'info');
    useAppStore.getState().showToast('world', 'error');
    const id = useAppStore.getState().toasts[0].id;
    useAppStore.getState().hideToast(id);
    const remaining = useAppStore.getState().toasts;
    expect(remaining).toHaveLength(1);
    expect(remaining[0].message).toBe('world');
  });

  // ── Settings ──

  it('setTheme changes the theme', () => {
    const { setTheme } = useAppStore.getState();

    setTheme('dark');
    expect(useAppStore.getState().theme).toBe('dark');

    setTheme('light');
    expect(useAppStore.getState().theme).toBe('light');

    setTheme('system');
    expect(useAppStore.getState().theme).toBe('system');
  });

  it('setFontSize changes the font size', () => {
    const { setFontSize } = useAppStore.getState();

    setFontSize('large');
    expect(useAppStore.getState().fontSize).toBe('large');

    setFontSize('small');
    expect(useAppStore.getState().fontSize).toBe('small');

    setFontSize('medium');
    expect(useAppStore.getState().fontSize).toBe('medium');
  });

  it('setApiBaseUrl updates apiBaseUrl', () => {
    useAppStore.getState().setApiBaseUrl('http://test:8080');
    expect(useAppStore.getState().apiBaseUrl).toBe('http://test:8080');
  });

  // ── Current Session ──

  it('setCurrentSession updates currentSessionId', () => {
    useAppStore.getState().setCurrentSession('session-abc');
    expect(useAppStore.getState().currentSessionId).toBe('session-abc');

    useAppStore.getState().setCurrentSession(null);
    expect(useAppStore.getState().currentSessionId).toBeNull();
  });

  // ── Model ──

  it('setModel updates selectedModel', () => {
    useAppStore.getState().setModel('gpt-4');
    expect(useAppStore.getState().selectedModel).toBe('gpt-4');

    useAppStore.getState().setModel(null);
    expect(useAppStore.getState().selectedModel).toBeNull();
  });

  // ── Configured ──

  it('setConfigured updates configured flag', () => {
    useAppStore.getState().setConfigured(true);
    expect(useAppStore.getState().configured).toBe(true);

    useAppStore.getState().setConfigured(false);
    expect(useAppStore.getState().configured).toBe(false);
  });

  // ── mergeServerMessages（任务终态感知：按 id 去重合并，追加语义） ──

  it('mergeServerMessages appends server-only messages and keeps local ones', () => {
    useAppStore.setState({
      currentSessionId: 'session-1',
      messages: [
        {
          id: 'msg-1',
          role: 'assistant',
          content: '本地累积的消息',
          timestamp: '2026-09-04T00:00:00Z',
        },
      ],
    });

    useAppStore.getState().mergeServerMessages('session-1', [
      {
        id: 'msg-1',
        role: 'assistant',
        content: '服务端同 id 消息（应被跳过，保留本地）',
        timestamp: '2026-09-04T00:00:00Z',
      },
      {
        id: 'msg_983',
        role: 'system',
        content: '[后台命令失败] cargo tauri build（cmd_1，退出码 1）',
        timestamp: '2026-09-04T00:01:00Z',
      },
      {
        id: 'msg_984',
        role: 'assistant',
        content: '唤醒轮汇总结果',
        timestamp: '2026-09-04T00:01:10Z',
      },
    ]);

    const msgs = useAppStore.getState().messages;
    expect(msgs.length).toBe(3);
    expect(msgs[0].id).toBe('msg-1');
    expect(msgs[0].content).toBe('本地累积的消息');
    expect(msgs[1].id).toBe('msg_983');
    expect(msgs[1].role).toBe('system');
    expect(msgs[2].id).toBe('msg_984');
  });

  it('mergeServerMessages updates the session cache for non-current sessions', () => {
    useAppStore.setState({
      currentSessionId: 'session-1',
      messages: [],
    });

    useAppStore.getState().mergeServerMessages('session-2', [
      {
        id: 'msg-2',
        role: 'system',
        content: '[后台任务完成] 其他会话的通知',
        timestamp: '2026-09-04T00:00:00Z',
      },
    ]);

    // 非当前会话：只更新字典，不污染当前投影
    expect(useAppStore.getState().messages).toEqual([]);
    expect(useAppStore.getState().sessionMessages['session-2']?.length).toBe(1);
  });
});