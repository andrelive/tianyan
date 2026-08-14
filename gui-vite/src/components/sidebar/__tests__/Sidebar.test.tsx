import { render, screen, waitFor, fireEvent, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import { server } from '@/test/mocks/server';
import { http, HttpResponse } from 'msw';
import Sidebar from '../Sidebar';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** A dummy component that renders the current location for route assertions. */
function LocationDisplay() {
  const location = useLocation();
  return <div data-testid="location-display">{location.pathname}</div>;
}

function renderSidebar(initialEntries: string[] = ['/chat']) {
  return render(
    <MemoryRouter initialEntries={initialEntries}>
      <Sidebar />
      <LocationDisplay />
    </MemoryRouter>,
  );
}

// ---------------------------------------------------------------------------
// Suite
// ---------------------------------------------------------------------------

describe('Sidebar', () => {
  beforeEach(() => {
    useAppStore.setState({
      sessions: [],
      currentSessionId: null,
      currentView: 'chat',
      isSidebarOpen: true,
      messages: [],
      toast: null,
    });
  });

  // ── Collapsed state ──────────────────────────────────────────────────────

  describe('Collapsed state (isSidebarOpen=false)', () => {
    beforeEach(() => {
      useAppStore.setState({ isSidebarOpen: false });
    });

    it('renders collapsed with 64px width', () => {
      renderSidebar();
      const aside = screen.getByRole('navigation', { name: '导航' });
      expect(aside.className).toContain('w-[64px]');
    });

    it('shows PanelLeft expand button', () => {
      renderSidebar();
      expect(screen.getByTitle('展开侧边栏')).toBeInTheDocument();
    });

    it('shows all NAV_ITEMS as icon buttons', () => {
      renderSidebar();
      expect(screen.getByTitle('对话')).toBeInTheDocument();
      expect(screen.getByTitle('技能')).toBeInTheDocument();
      expect(screen.getByTitle('知识')).toBeInTheDocument();
      expect(screen.getByTitle('设置')).toBeInTheDocument();
    });

    it('clicking expand button calls toggleSidebar', async () => {
      const user = userEvent.setup();
      renderSidebar();

      await user.click(screen.getByTitle('展开侧边栏'));

      expect(useAppStore.getState().isSidebarOpen).toBe(true);
    });

    it('clicking nav icons updates view and navigates', async () => {
      const user = userEvent.setup();
      renderSidebar();

      await user.click(screen.getByTitle('技能'));
      expect(useAppStore.getState().currentView).toBe('skills');
      expect(screen.getByTestId('location-display').textContent).toBe('/skills');

      await user.click(screen.getByTitle('设置'));
      expect(useAppStore.getState().currentView).toBe('settings');
      expect(screen.getByTestId('location-display').textContent).toBe('/settings');
    });
  });

  // ── Expanded state ───────────────────────────────────────────────────────

  describe('Expanded state (isSidebarOpen=true)', () => {
    it('renders expanded with 280px width', () => {
      renderSidebar();
      const aside = screen.getByRole('navigation', { name: '导航' });
      expect(aside.className).toContain('w-[280px]');
    });

    it('shows header with "天演" title', () => {
      renderSidebar();
      expect(screen.getByText('天演')).toBeInTheDocument();
    });

    it('shows Plus (new chat) and PanelLeftClose (collapse) buttons', () => {
      renderSidebar();
      expect(screen.getByTitle('新建对话')).toBeInTheDocument();
      expect(screen.getByTitle('收起侧边栏')).toBeInTheDocument();
    });

    it('shows NAV_ITEMS with labels', () => {
      renderSidebar();
      expect(screen.getByText('对话')).toBeInTheDocument();
      expect(screen.getByText('技能')).toBeInTheDocument();
      expect(screen.getByText('知识')).toBeInTheDocument();
      expect(screen.getByText('设置')).toBeInTheDocument();
    });

    it('clicking "新建对话" opens the workspace picker without resetting state', async () => {
      const user = userEvent.setup();
      useAppStore.setState({ currentSessionId: 'session-1' });
      renderSidebar();

      await user.click(screen.getByTitle('新建对话'));

      // 先选工作区（工作区 = 会话的父级分组），选择完成前不重置当前状态
      expect(screen.getByRole('dialog', { name: '选择工作目录' })).toBeInTheDocument();
      expect(useAppStore.getState().currentSessionId).toBe('session-1');
    });

    it('choosing "不绑定工作区" resets session, messages, view, and navigates to /chat', async () => {
      const user = userEvent.setup();
      // Pre-set state to verify it gets reset
      useAppStore.setState({
        currentSessionId: 'session-1',
        messages: [{ role: 'user', content: 'hello', timestamp: '2026-01-01T00:00:00Z' }],
        currentView: 'settings',
      });
      renderSidebar();

      await user.click(screen.getByTitle('新建对话'));
      await user.click(screen.getByRole('button', { name: '不绑定工作区' }));

      expect(useAppStore.getState().newSessionWorkspace).toBeNull();
      expect(useAppStore.getState().currentSessionId).toBeNull();
      expect(useAppStore.getState().messages).toEqual([]);
      expect(useAppStore.getState().currentView).toBe('chat');
      expect(screen.getByTestId('location-display').textContent).toBe('/chat');
    });

    it('choosing a directory sets newSessionWorkspace and navigates to /chat', async () => {
      const user = userEvent.setup();
      renderSidebar();

      await user.click(screen.getByTitle('新建对话'));
      const dialog = screen.getByRole('dialog', { name: '选择工作目录' });
      // 浏览根列出盘符 → 双击 C:\ 进入二级浏览 → 选中子目录 → 确认
      await user.dblClick(within(dialog).getByRole('button', { name: '目录 C:\\' }));
      // 二级浏览为异步加载：等待子目录出现后选中
      const subDir = await within(dialog).findByRole('button', { name: '目录 sub1' });
      await user.click(subDir);
      await user.click(within(dialog).getByRole('button', { name: '确认选择' }));

      expect(useAppStore.getState().newSessionWorkspace).toBe('C:\\sub1');
      expect(useAppStore.getState().currentSessionId).toBeNull();
      expect(screen.getByTestId('location-display').textContent).toBe('/chat');
    });

    it('clicking collapse button toggles sidebar closed', async () => {
      const user = userEvent.setup();
      renderSidebar();

      await user.click(screen.getByTitle('收起侧边栏'));

      expect(useAppStore.getState().isSidebarOpen).toBe(false);
    });
  });

  // ── Session list ─────────────────────────────────────────────────────────

  describe('Session list', () => {
    it('shows empty state when sessions=[]', () => {
      useAppStore.setState({ sessions: [] });
      renderSidebar();
      expect(screen.getByText('暂无会话')).toBeInTheDocument();
    });

    it('shows session list when sessions exist', async () => {
      // Do NOT pre-populate — let the useEffect GET response populate
      // sessions so we can wait for the async fetch to settle.
      renderSidebar();
      await waitFor(() => {
        expect(screen.getByText('测试会话 1')).toBeInTheDocument();
      });
      expect(screen.getByText('代码审查对话')).toBeInTheDocument();
    });

    it('clicking a session navigates to /chat/{session.id}', async () => {
      const user = userEvent.setup();
      renderSidebar();

      await waitFor(() => {
        expect(screen.getByText('测试会话 1')).toBeInTheDocument();
      });

      await user.click(screen.getByText('测试会话 1'));

      expect(useAppStore.getState().currentSessionId).toBe('session-1');
      expect(useAppStore.getState().currentView).toBe('chat');
      expect(screen.getByTestId('location-display').textContent).toBe('/chat/session-1');
    });

    it('hovering a session reveals the delete button', async () => {
      const user = userEvent.setup();
      renderSidebar();

      await waitFor(() => {
        expect(screen.getByText('测试会话 1')).toBeInTheDocument();
      });

      const sessionTitle = screen.getByText('测试会话 1');
      const sessionRow = sessionTitle.closest('.group')!;
      await user.hover(sessionRow);

      await waitFor(() => {
        expect(screen.getByTitle('删除会话')).toBeInTheDocument();
      });
    });

    it('clicking delete succeeds and calls removeSession', async () => {
      const user = userEvent.setup();
      renderSidebar();

      // Wait for sessions to appear in the DOM (the GET response populated
      // the store AND React committed the re-render).
      await waitFor(() => {
        expect(screen.getByText('测试会话 1')).toBeInTheDocument();
      });

      // Hover the session row to reveal the delete button.
      // Use fireEvent for the non-bubbling mouseEnter to ensure it hits
      // the element with the onMouseEnter handler.
      const sessionRow = screen.getByText('测试会话 1').closest('.group')!;
      fireEvent.mouseEnter(sessionRow);

      await waitFor(() => {
        expect(screen.getByTitle('删除会话')).toBeInTheDocument();
      });

      await user.click(screen.getByTitle('删除会话'));

      // Wait for async deletion — the session should be removed
      await waitFor(() => {
        const sessions = useAppStore.getState().sessions;
        expect(sessions).toHaveLength(1);
        expect(sessions[0].id).toBe('session-2');
      });
    });

    it('delete failure shows error toast', async () => {
      // Override DELETE handler to return 500
      server.use(
        http.delete('/api/v1/sessions/:id', () => {
          return new HttpResponse(null, { status: 500 });
        }),
      );

      const user = userEvent.setup();
      renderSidebar();

      // Wait for sessions to appear in the DOM.
      await waitFor(() => {
        expect(screen.getByText('测试会话 1')).toBeInTheDocument();
      });

      // Hover to reveal delete button
      const sessionRow = screen.getByText('测试会话 1').closest('.group')!;
      fireEvent.mouseEnter(sessionRow);

      await waitFor(() => {
        expect(screen.getByTitle('删除会话')).toBeInTheDocument();
      });

      await user.click(screen.getByTitle('删除会话'));

      await waitFor(() => {
        const toast = useAppStore.getState().toast;
        expect(toast).not.toBeNull();
        expect(toast!.message).toBe('删除会话失败');
        expect(toast!.type).toBe('error');
      });
    });
  });

  // ── Session rename ───────────────────────────────────────────────────────

  describe('Session rename', () => {
    const renameSession = {
      id: 'session-1',
      title: '旧标题',
      created_at: '2026-07-20T10:00:00Z',
      updated_at: '2026-07-20T10:00:00Z',
      message_count: 0,
    };

    beforeEach(() => {
      // 覆盖 GET /sessions，避免 mount 后的 fetch 用默认 mock 数据覆盖预置会话
      server.use(
        http.get('/api/v1/sessions', () => {
          return HttpResponse.json({ sessions: [renameSession], total: 1 });
        }),
      );
      useAppStore.setState({ sessions: [renameSession] });
    });

    it('double-clicking a title shows an inline editor', async () => {
      const user = userEvent.setup();
      renderSidebar();

      await waitFor(() => {
        expect(screen.getByText('旧标题')).toBeInTheDocument();
      });

      await user.dblClick(screen.getByText('旧标题'));
      expect(screen.getByLabelText('编辑会话标题')).toBeInTheDocument();
    });

    it('submitting a new title updates the store via the title API', async () => {
      const user = userEvent.setup();
      renderSidebar();

      await waitFor(() => {
        expect(screen.getByText('旧标题')).toBeInTheDocument();
      });

      await user.dblClick(screen.getByText('旧标题'));
      const input = screen.getByLabelText('编辑会话标题');
      await user.clear(input);
      await user.type(input, '新标题');
      await user.keyboard('{Enter}');

      await waitFor(() => {
        const sessions = useAppStore.getState().sessions;
        expect(sessions[0].title).toBe('新标题');
      });
      expect(screen.getByText('新标题')).toBeInTheDocument();
    });
  });

  // ── Navigation ───────────────────────────────────────────────────────────

  describe('Navigation', () => {
    it('highlights the correct nav item based on active route', () => {
      // Active nav styling is driven by location.pathname, not currentView.
      renderSidebar(['/skills']);

      // The <button> element for "技能" should have the active class
      // (getByText returns the <span> child; we need the parent <button>).
      const skillsBtn = screen.getByText('技能').closest('button')!;
      expect(skillsBtn.className).toContain('text-blue-700');

      // Chat should NOT have the active class
      const chatBtn = screen.getByText('对话').closest('button')!;
      expect(chatBtn.className).not.toContain('text-blue-700');
    });

    it('clicking a nav item sets view and navigates', async () => {
      const user = userEvent.setup();
      renderSidebar();

      await user.click(screen.getByText('技能'));
      expect(useAppStore.getState().currentView).toBe('skills');
      expect(screen.getByTestId('location-display').textContent).toBe('/skills');

      await user.click(screen.getByText('设置'));
      expect(useAppStore.getState().currentView).toBe('settings');
      expect(screen.getByTestId('location-display').textContent).toBe('/settings');
    });
  });

  // ── Bug fix verification ─────────────────────────────────────────────────

  describe('Bug fix: ListSessionsResponse format', () => {
    it('does not crash when API returns empty ListSessionsResponse', async () => {
      // Override the GET handler to return a well-formed ListSessionsResponse
      // with an empty array, ensuring the component accesses data.sessions
      // instead of treating the raw response as an array.
      server.use(
        http.get('/api/v1/sessions', () => {
          return HttpResponse.json({ sessions: [], total: 0 });
        }),
      );

      useAppStore.setState({ sessions: ['anything' as unknown as never] });
      renderSidebar();

      // After the fetch fires and setSessions(data.sessions) runs, sessions
      // should be the empty array from the response — no crash from .map().
      await waitFor(() => {
        expect(useAppStore.getState().sessions).toEqual([]);
      });

      // The component should now render the empty state.
      expect(screen.getByText('暂无会话')).toBeInTheDocument();
    });
  });
});
