import { describe, it, expect, beforeEach } from 'vitest';
import { useAppStore, PENDING_SESSION_KEY } from '@/lib/store';
import type { ChatMessage, Session, SkillCallInfo } from '@/lib/types';
import { messageText } from '@/lib/types';

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
      messages: [{ role: 'user', segments: [{ type: 'text', text: 'hello' }] }],
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
      messages: [{ role: 'assistant', segments: [{ type: 'text', text: 'keep' }] }],
    });

    useAppStore.getState().removeSession('1');
    expect(useAppStore.getState().sessions).toHaveLength(1);
    expect(useAppStore.getState().currentSessionId).toBe('2');
    expect(useAppStore.getState().messages).toHaveLength(1);
  });

  // ── Messages ──

  it('setSessionMessages replaces the session cache and syncs the current projection', () => {
    useAppStore.setState({ currentSessionId: 'session-1' });
    const msgs: ChatMessage[] = [{ role: 'user', segments: [{ type: 'text', text: 'first' }] }];

    useAppStore.getState().setSessionMessages('session-1', msgs);
    expect(useAppStore.getState().sessionMessages['session-1']).toHaveLength(1);
    expect(useAppStore.getState().messages).toHaveLength(1);
    expect(messageText(useAppStore.getState().messages[0])).toBe('first');
  });

  it('setSessionMessages writes only the target session (no implicit current session)', () => {
    // 显式寻址：目标非当前会话时只写字典，不污染当前投影
    useAppStore.setState({ currentSessionId: 'session-A' });
    const msgs: ChatMessage[] = [{ role: 'user', segments: [{ type: 'text', text: 'B' }] }];

    useAppStore.getState().setSessionMessages('session-B', msgs);
    expect(useAppStore.getState().sessionMessages['session-B']).toHaveLength(1);
    expect(useAppStore.getState().messages).toHaveLength(0);
  });

  it('updateLastMessage never writes into a non-assistant trailing message (role guard)', () => {
    // role 守卫：列表末条为 system（后台通知落尾）/user 时不得写入增量
    useAppStore.setState({
      messages: [
        { role: 'assistant', segments: [{ type: 'text', text: '回答' }] },
        { role: 'system', segments: [{ type: 'text', text: '[通知]' }] },
      ],
    });

    useAppStore.getState().updateLastMessage('增量');
    const msgs = useAppStore.getState().messages;
    expect(messageText(msgs[0])).toBe('回答增量');
    expect(messageText(msgs[1])).toBe('[通知]');
  });

  it('addMessage appends a message', () => {
    useAppStore.getState().addMessage({ role: 'user', segments: [{ type: 'text', text: 'q' }] });
    useAppStore
      .getState()
      .addMessage({ role: 'assistant', segments: [{ type: 'text', text: 'a' }] });

    const msgs = useAppStore.getState().messages;
    expect(msgs).toHaveLength(2);
    expect(msgs[0].role).toBe('user');
    expect(msgs[1].role).toBe('assistant');
  });

  it('updateLastMessage appends delta to the last message content', () => {
    useAppStore.setState({
      messages: [
        { role: 'user', segments: [{ type: 'text', text: 'hi' }] },
        { role: 'assistant', segments: [{ type: 'text', text: 'Hel' }] },
      ],
    });

    useAppStore.getState().updateLastMessage('lo');
    expect(messageText(useAppStore.getState().messages[1])).toBe('Hello');
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
        { role: 'user', segments: [{ type: 'text', text: 'hi' }] },
        { role: 'assistant', segments: [{ type: 'text', text: '完成回答' }], tool_calls: [] },
      ],
    });
    useAppStore.getState().startNewAssistantTurn();
    const msgs = useAppStore.getState().messages;
    expect(msgs).toHaveLength(3);
    expect(msgs[2].role).toBe('assistant');
    expect(messageText(msgs[2])).toBe('');
  });

  it('startNewAssistantTurn reuses an empty placeholder', () => {
    // 空占位（流式初始）→ 复用，不新开
    useAppStore.setState({
      messages: [
        { role: 'user', segments: [{ type: 'text', text: 'hi' }] },
        { role: 'assistant', segments: [], tool_calls: [] },
      ],
    });
    useAppStore.getState().startNewAssistantTurn();
    expect(useAppStore.getState().messages).toHaveLength(2);
  });

  it('clearMessages empties the messages array', () => {
    useAppStore.setState({ messages: [{ role: 'user', segments: [{ type: 'text', text: 'x' }] }] });

    useAppStore.getState().clearMessages();
    expect(useAppStore.getState().messages).toEqual([]);
  });

  it('deleteMessagesFrom removes messages from index and resets stream status', () => {
    useAppStore.setState({
      messages: [
        { role: 'user', segments: [{ type: 'text', text: 'a' }] },
        { role: 'assistant', segments: [{ type: 'text', text: 'b' }] },
        { role: 'user', segments: [{ type: 'text', text: 'c' }] },
      ],
      streamStatus: { [PENDING_SESSION_KEY]: 'streaming' },
    });

    useAppStore.getState().deleteMessagesFrom(PENDING_SESSION_KEY, 1);
    expect(useAppStore.getState().messages).toHaveLength(1);
    expect(messageText(useAppStore.getState().messages[0])).toBe('a');
    expect(useAppStore.getState().streamStatus).toEqual({ [PENDING_SESSION_KEY]: 'idle' });
  });

  it('setTurnError sets and clears the per-session turn error (front-end only)', () => {
    // 轮级失败横幅：纯前端内存态（不落库）——设置/清除/幂等
    useAppStore.getState().setTurnError('session-A', 'LLM 请求失败（已重试）');
    expect(useAppStore.getState().turnErrorBySession['session-A']).toBe('LLM 请求失败（已重试）');

    useAppStore.getState().setTurnError('session-A', null);
    expect(useAppStore.getState().turnErrorBySession['session-A']).toBeUndefined();
    // 幂等：重复清除不报错
    useAppStore.getState().setTurnError('session-A', null);
    expect(useAppStore.getState().turnErrorBySession['session-A']).toBeUndefined();
  });
  it('setStopping sets and clears the per-session stopping flag (front-end only)', () => {
    // 「正在停止」过渡态：纯前端内存态（不落库）——设置/清除/幂等
    useAppStore.getState().setStopping('session-A', true);
    expect(useAppStore.getState().stoppingBySession['session-A']).toBe(true);

    useAppStore.getState().setStopping('session-A', false);
    expect(useAppStore.getState().stoppingBySession['session-A']).toBeUndefined();
    // 幂等：重复清除不报错
    useAppStore.getState().setStopping('session-A', false);
    expect(useAppStore.getState().stoppingBySession['session-A']).toBeUndefined();
  });

  // ── Skill Calls ──

  it('appendSkillCalls sets skill_calls on the last assistant message', () => {
    useAppStore.setState({
      messages: [
        { role: 'user', segments: [{ type: 'text', text: 'go' }] },
        { role: 'assistant', segments: [{ type: 'text', text: 'searching' }], skill_calls: [] },
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
      messages: [{ role: 'user', segments: [{ type: 'text', text: 'hi' }] }],
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
        { role: 'user', segments: [{ type: 'text', text: 'go' }] },
        { role: 'assistant', segments: [{ type: 'text', text: 'working' }] },
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
      messages: [{ role: 'user', segments: [{ type: 'text', text: 'hi' }] }],
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
        { role: 'user', segments: [{ type: 'text', text: 'hi' }] },
        { role: 'assistant', segments: [] },
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

  // ── Input drafts（每会话输入草稿；ChatInput 页面/会话切换恢复）──

  it('setInputDraft stores per-session drafts, supports functional updates and drops empty ones', () => {
    useAppStore.getState().setInputDraft('s1', { text: '草稿一', images: [] });
    expect(useAppStore.getState().inputDrafts['s1']).toEqual({ text: '草稿一', images: [] });

    // 函数式更新：基于最新状态合并（异步回调场景不覆盖并发输入）
    useAppStore
      .getState()
      .setInputDraft('s1', (prev) => ({ ...prev, images: ['data:image/png;base64,x'] }));
    expect(useAppStore.getState().inputDrafts['s1']).toEqual({
      text: '草稿一',
      images: ['data:image/png;base64,x'],
    });

    // 空草稿删除键（发送后清空——不残留空条目）
    useAppStore.getState().setInputDraft('s1', { text: '', images: [] });
    expect('s1' in useAppStore.getState().inputDrafts).toBe(false);
  });

  // ── Model ──

  it('setModel updates selectedModel', () => {
    useAppStore.getState().setModel({ provider: 'x', model: 'gpt-4' });
    expect(useAppStore.getState().selectedModel).toEqual({ provider: 'x', model: 'gpt-4' });

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

  // ── appendThinking（思考增量合并） ──

  it('appendThinking merges consecutive thinking deltas into one segment', () => {
    useAppStore.setState({
      currentSessionId: 'session-1',
      sessionMessages: {
        'session-1': [
          { id: 'msg-1', role: 'user', segments: [{ type: 'text', text: '问题' }], timestamp: '' },
          { id: 'msg-2', role: 'assistant', segments: [], timestamp: '' },
        ],
      },
      messages: [
        { id: 'msg-1', role: 'user', segments: [{ type: 'text', text: '问题' }], timestamp: '' },
        { id: 'msg-2', role: 'assistant', segments: [], timestamp: '' },
      ],
    });

    const { appendThinking } = useAppStore.getState();
    appendThinking('先分析', 'session-1');
    appendThinking('再想想', 'session-1');
    appendThinking('得出结论', 'session-1');

    const assistant = useAppStore.getState().messages[1];
    expect(assistant.thinking).toBe('先分析再想想得出结论');
    // 连续 thinking 增量合并为单个段（O(1) 追加，不逐 delta 膨胀）
    expect(assistant.segments).toEqual([{ type: 'thinking', text: '先分析再想想得出结论' }]);
  });

  it('appendThinking starts a new segment after a non-thinking segment', () => {
    useAppStore.setState({
      currentSessionId: 'session-1',
      sessionMessages: {
        'session-1': [
          { id: 'msg-1', role: 'user', segments: [{ type: 'text', text: '问题' }], timestamp: '' },
          {
            id: 'msg-2',
            role: 'assistant',
            segments: [{ type: 'text', text: '正文' }],
            timestamp: '',
          },
        ],
      },
      messages: [
        { id: 'msg-1', role: 'user', segments: [{ type: 'text', text: '问题' }], timestamp: '' },
        {
          id: 'msg-2',
          role: 'assistant',
          segments: [{ type: 'text', text: '正文' }],
          timestamp: '',
        },
      ],
    });

    useAppStore.getState().appendThinking('补充思考', 'session-1');

    const assistant = useAppStore.getState().messages[1];
    expect(assistant.segments).toEqual([
      { type: 'text', text: '正文' },
      { type: 'thinking', text: '补充思考' },
    ]);
  });
});
