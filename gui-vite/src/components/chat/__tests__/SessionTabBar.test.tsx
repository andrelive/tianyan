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
    recentSessionIds: [],
    sessions: [],
    sessionMessages: {},
    streamStatus: {},
    turnState: {},
    backgroundTasks: [],
  });
});

describe('SessionTabBar', () => {
  it('renders MRU sessions in order, current highlighted', () => {
    useAppStore.setState({
      currentSessionId: 's1',
      recentSessionIds: ['s1', 's2'],
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
    expect(tabs[0]).toHaveAttribute('aria-selected', 'true');
    expect(tabs[1]).toHaveTextContent('会话二');
    expect(tabs[1]).toHaveAttribute('aria-selected', 'false');
  });

  it('filters out sessions that no longer exist', () => {
    useAppStore.setState({
      recentSessionIds: ['ghost', 's1'],
      sessions: [makeSession({ id: 's1', title: '会话一' })],
    });
    renderTabBar();
    expect(screen.getAllByRole('tab')).toHaveLength(1);
    expect(screen.getByRole('tab', { name: /会话一/ })).toBeInTheDocument();
  });

  it('renders nothing when there are no recent sessions', () => {
    renderTabBar();
    expect(screen.queryByRole('tablist')).not.toBeInTheDocument();
  });

  it('clicking a tab switches session and navigates', async () => {
    useAppStore.setState({
      currentSessionId: 's1',
      recentSessionIds: ['s2', 's1'],
      sessions: [
        makeSession({ id: 's1', title: '会话一' }),
        makeSession({ id: 's2', title: '会话二' }),
      ],
      sessionMessages: { s2: [cachedMessage] },
    });
    const user = userEvent.setup();
    renderTabBar();

    await user.click(screen.getByRole('tab', { name: /会话二/ }));
    expect(useAppStore.getState().currentSessionId).toBe('s2');
    await waitFor(() => {
      expect(screen.getByTestId('location-display')).toHaveTextContent('/chat/s2');
    });
  });

  it('shows running status mark from turn state', () => {
    useAppStore.setState({
      recentSessionIds: ['s1'],
      sessions: [makeSession({ id: 's1', title: '会话一' })],
      turnState: { s1: { state: 'running', auto: false } },
    });
    renderTabBar();
    expect(screen.getByLabelText('运行中')).toBeInTheDocument();
  });
});
