import { describe, it, expect, beforeEach } from 'vitest';
import { useAppStore } from '@/lib/store';
import type { ChatMessage, Session, Skill, SkillCallInfo } from '@/lib/types';

describe('useAppStore', () => {
  // Reset store to initial values before each test
  beforeEach(() => {
    useAppStore.setState({
      currentView: 'chat',
      currentSessionId: null,
      sessions: [],
      messages: [],
      streamStatus: 'idle',
      isSidebarOpen: true,
      theme: 'system',
      fontSize: 'medium',
      apiBaseUrl: 'http://localhost:3000',
      skills: [],
      currentSkillId: null,
      toast: null,
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
    expect(state.streamStatus).toBe('idle');
    expect(state.isSidebarOpen).toBe(true);
    expect(state.theme).toBe('system');
    expect(state.fontSize).toBe('medium');
    expect(state.apiBaseUrl).toBe('http://localhost:3000');
    expect(state.skills).toEqual([]);
    expect(state.currentSkillId).toBeNull();
    expect(state.toast).toBeNull();
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

  // ── Sidebar ──

  it('toggleSidebar flips isSidebarOpen', () => {
    expect(useAppStore.getState().isSidebarOpen).toBe(true);

    useAppStore.getState().toggleSidebar();
    expect(useAppStore.getState().isSidebarOpen).toBe(false);

    useAppStore.getState().toggleSidebar();
    expect(useAppStore.getState().isSidebarOpen).toBe(true);
  });

  it('setSidebarOpen sets isSidebarOpen explicitly', () => {
    useAppStore.getState().setSidebarOpen(false);
    expect(useAppStore.getState().isSidebarOpen).toBe(false);

    useAppStore.getState().setSidebarOpen(true);
    expect(useAppStore.getState().isSidebarOpen).toBe(true);
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
    const s: Session = { id: 'cur', title: 'Current', created_at: '', updated_at: '', message_count: 0 };
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

  it('clearMessages empties the messages array', () => {
    useAppStore.setState({ messages: [{ role: 'user', content: 'x' }] });

    useAppStore.getState().clearMessages();
    expect(useAppStore.getState().messages).toEqual([]);
  });

  it('deleteMessagesFrom removes messages from index onward and resets stream status', () => {
    useAppStore.setState({
      messages: [
        { role: 'user', content: 'a' },
        { role: 'assistant', content: 'b' },
        { role: 'user', content: 'c' },
      ],
      streamStatus: 'streaming',
    });

    useAppStore.getState().deleteMessagesFrom(1);
    expect(useAppStore.getState().messages).toHaveLength(1);
    expect(useAppStore.getState().messages[0].content).toBe('a');
    expect(useAppStore.getState().streamStatus).toBe('idle');
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
    useAppStore.getState().appendSkillCalls([
      { skill_id: 's1', skill_name: 't', success: true, execution_time_ms: 0 },
    ]);
    expect(useAppStore.getState().messages).toHaveLength(1);
    expect(useAppStore.getState().messages[0].skill_calls).toBeUndefined();
  });

  // ── Streaming ──

  it('setStreamStatus updates streamStatus', () => {
    const { setStreamStatus } = useAppStore.getState();

    setStreamStatus('streaming');
    expect(useAppStore.getState().streamStatus).toBe('streaming');

    setStreamStatus('error');
    expect(useAppStore.getState().streamStatus).toBe('error');

    setStreamStatus('idle');
    expect(useAppStore.getState().streamStatus).toBe('idle');
  });

  // ── Toast ──

  it('showToast sets the toast', () => {
    useAppStore.getState().showToast('Something failed', 'error');
    expect(useAppStore.getState().toast).toEqual({
      message: 'Something failed',
      type: 'error',
    });

    useAppStore.getState().showToast('Done!', 'success');
    expect(useAppStore.getState().toast).toEqual({
      message: 'Done!',
      type: 'success',
    });

    useAppStore.getState().showToast('FYI', 'info');
    expect(useAppStore.getState().toast).toEqual({
      message: 'FYI',
      type: 'info',
    });
  });

  it('hideToast clears the toast', () => {
    useAppStore.setState({ toast: { message: 'hello', type: 'info' } });
    useAppStore.getState().hideToast();
    expect(useAppStore.getState().toast).toBeNull();
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

  // ── Skills ──

  it('setSkills replaces the skills array', () => {
    const skills: Skill[] = [
      { id: 'x', name: 'X', description: '', parameters: [], category: '' },
    ];

    useAppStore.getState().setSkills(skills);
    expect(useAppStore.getState().skills).toHaveLength(1);
    expect(useAppStore.getState().skills[0].id).toBe('x');
  });

  it('setCurrentSkillId updates currentSkillId', () => {
    useAppStore.getState().setCurrentSkillId('skill-42');
    expect(useAppStore.getState().currentSkillId).toBe('skill-42');

    useAppStore.getState().setCurrentSkillId(null);
    expect(useAppStore.getState().currentSkillId).toBeNull();
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
});
