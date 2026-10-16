import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import SessionTabBar from '@/components/chat/SessionTabBar';
import type { ChatMessage, Session } from '@/lib/types';

function LocationDisplay() {
  const location = useLocation();
  return <div data-testid="location-display">{location.pathname}</div>;
}

function renderTabBar() {
  return render(
    <MemoryRouter initialEntries={['/chat']}>
      <SessionTabBar />
      <LocationDisplay />
    </MemoryRouter>,
  );
}

/** 会话 fixture（对齐 SessionList 测试的构造方式）。 */
function makeSession(overrides: Partial<Session> & { id: string; title: string }): Session {
  return {
    created_at: '2026-07-20T10:00:00Z',
    updated_at: '2026-07-23T08:00:00Z',
    message_count: 3,
    working_directory: null,
    ...overrides,
  };
}

/** 预置缓存消息：让 loadSessionHistory 走缓存命中（不发请求）。 */
const cachedMessage: ChatMessage = { role: 'user', segments: [] };

beforeEach(() => {
  useAppStore.setState({
    currentSessionId: null,
    openSessionIds: [],
    sessions: [],
    sessionMessages: {},
    streamStatus: {},
    turnState: {},
    backgroundTasks: [],
  });
});

describe('SessionTabBar', () => {
  it('renders open sessions in open order (append-only), current highlighted', () => {
    useAppStore.setState({
      currentSessionId: 's2',
      openSessionIds: ['s1', 's2'],
      sessions: [
        makeSession({ id: 's1', title: '会话一' }),
        makeSession({ id: 's2', title: '会话二' }),
        makeSession({ id: 's3', title: '未打开' }),
      ],
    });
    renderTabBar();

    const tabs = screen.getAllByRole('tab');
    expect(tabs).toHaveLength(2);
    expect(tabs[0]).toHaveTextContent('会话一');
    expect(tabs[0]).toHaveAttribute('aria-selected', 'false');
    expect(tabs[1]).toHaveTextContent('会话二');
    expect(tabs[1]).toHaveAttribute('aria-selected', 'true');
  });

  it('filters out sessions that no longer exist', () => {
    useAppStore.setState({
      openSessionIds: ['ghost', 's1'],
      sessions: [makeSession({ id: 's1', title: '会话一' })],
    });
    renderTabBar();
    expect(screen.getAllByRole('tab')).toHaveLength(1);
    expect(screen.getByRole('tab', { name: /会话一/ })).toBeInTheDocument();
  });

  it('renders nothing when there are no open sessions', () => {
    renderTabBar();
    expect(screen.queryByRole('tablist')).not.toBeInTheDocument();
  });

  it('clicking a tab switches session and navigates, without reordering', async () => {
    useAppStore.setState({
      currentSessionId: 's2',
      openSessionIds: ['s1', 's2'],
      sessions: [
        makeSession({ id: 's1', title: '会话一' }),
        makeSession({ id: 's2', title: '会话二' }),
      ],
      sessionMessages: { s1: [cachedMessage] },
    });
    const user = userEvent.setup();
    renderTabBar();

    await user.click(screen.getByRole('tab', { name: /会话一/ }));
    expect(useAppStore.getState().currentSessionId).toBe('s1');
    // 浏览器 tab 语义：切换不重排（打开顺序保持不变）
    expect(useAppStore.getState().openSessionIds).toEqual(['s1', 's2']);
    await waitFor(() => {
      expect(screen.getByTestId('location-display')).toHaveTextContent('/chat/s1');
    });
  });

  it('shows running status mark from turn state', () => {
    useAppStore.setState({
      openSessionIds: ['s1'],
      sessions: [makeSession({ id: 's1', title: '会话一' })],
      turnState: { s1: { state: 'running', auto: false } },
    });
    renderTabBar();
    expect(screen.getByLabelText('运行中')).toBeInTheDocument();
  });

  it('closing an inactive tab removes it without switching', async () => {
    useAppStore.setState({
      currentSessionId: 's1',
      openSessionIds: ['s1', 's2', 's3'],
      sessions: [
        makeSession({ id: 's1', title: '会话一' }),
        makeSession({ id: 's2', title: '会话二' }),
        makeSession({ id: 's3', title: '会话三' }),
      ],
    });
    const user = userEvent.setup();
    renderTabBar();

    await user.click(screen.getByRole('button', { name: '关闭 会话二' }));
    expect(useAppStore.getState().openSessionIds).toEqual(['s1', 's3']);
    expect(useAppStore.getState().currentSessionId).toBe('s1');
    // 未发生导航（仍停留原路由）
    expect(screen.getByTestId('location-display')).toHaveTextContent('/chat');
  });

  it('closing the active tab activates the right neighbor (browser behavior)', async () => {
    useAppStore.setState({
      currentSessionId: 's2',
      openSessionIds: ['s1', 's2', 's3'],
      sessions: [
        makeSession({ id: 's1', title: '会话一' }),
        makeSession({ id: 's2', title: '会话二' }),
        makeSession({ id: 's3', title: '会话三' }),
      ],
      sessionMessages: { s3: [cachedMessage] },
    });
    const user = userEvent.setup();
    renderTabBar();

    await user.click(screen.getByRole('button', { name: '关闭 会话二' }));
    expect(useAppStore.getState().openSessionIds).toEqual(['s1', 's3']);
    expect(useAppStore.getState().currentSessionId).toBe('s3');
    await waitFor(() => {
      expect(screen.getByTestId('location-display')).toHaveTextContent('/chat/s3');
    });
  });

  it('closing the active last tab falls back to the left neighbor', async () => {
    useAppStore.setState({
      currentSessionId: 's3',
      openSessionIds: ['s1', 's2', 's3'],
      sessions: [
        makeSession({ id: 's1', title: '会话一' }),
        makeSession({ id: 's2', title: '会话二' }),
        makeSession({ id: 's3', title: '会话三' }),
      ],
      sessionMessages: { s2: [cachedMessage] },
    });
    const user = userEvent.setup();
    renderTabBar();

    await user.click(screen.getByRole('button', { name: '关闭 会话三' }));
    expect(useAppStore.getState().openSessionIds).toEqual(['s1', 's2']);
    expect(useAppStore.getState().currentSessionId).toBe('s2');
    await waitFor(() => {
      expect(screen.getByTestId('location-display')).toHaveTextContent('/chat/s2');
    });
  });

  it('hides the close button when only one tab remains', () => {
    useAppStore.setState({
      currentSessionId: 's1',
      openSessionIds: ['s1'],
      sessions: [makeSession({ id: 's1', title: '会话一' })],
    });
    renderTabBar();
    expect(screen.getByRole('tab', { name: /会话一/ })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /关闭/ })).not.toBeInTheDocument();
  });
});
