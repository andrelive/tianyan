import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
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
      expect(screen.getByTitle('会话')).toBeInTheDocument();
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

    it('shows Plus (new session) and PanelLeftClose (collapse) buttons', () => {
      renderSidebar();
      expect(screen.getByTitle('新建会话')).toBeInTheDocument();
      expect(screen.getByTitle('收起侧边栏')).toBeInTheDocument();
    });

    it('shows NAV_ITEMS with labels', () => {
      renderSidebar();
      expect(screen.getByText('会话')).toBeInTheDocument();
      expect(screen.getByText('技能')).toBeInTheDocument();
      expect(screen.getByText('知识')).toBeInTheDocument();
      expect(screen.getByText('设置')).toBeInTheDocument();
    });

    it('does not show the old 工作区 nav item or session list', () => {
      renderSidebar();
      // 会话列表已迁入会话页左栏（SessionList）；全局侧边栏为纯一级导航
      expect(screen.queryByText('工作区')).not.toBeInTheDocument();
      expect(screen.queryByText('暂无会话')).not.toBeInTheDocument();
    });

    it('clicking "新建会话" resets session/messages/view and navigates to /chat', async () => {
      const user = userEvent.setup();
      // Pre-set state to verify it gets reset（零弹窗：直接进入会话页空状态）
      useAppStore.setState({
        currentSessionId: 'session-1',
        messages: [{ role: 'user', content: 'hello', timestamp: '2026-01-01T00:00:00Z' }],
        currentView: 'settings',
        newSessionWorkspace: 'C:/old-dir',
      });
      renderSidebar();

      await user.click(screen.getByTitle('新建会话'));

      expect(useAppStore.getState().currentSessionId).toBeNull();
      expect(useAppStore.getState().messages).toEqual([]);
      expect(useAppStore.getState().currentView).toBe('chat');
      expect(useAppStore.getState().newSessionWorkspace).toBeNull();
      expect(screen.getByTestId('location-display').textContent).toBe('/chat');
    });

    it('clicking collapse button toggles sidebar closed', async () => {
      const user = userEvent.setup();
      renderSidebar();

      await user.click(screen.getByTitle('收起侧边栏'));

      expect(useAppStore.getState().isSidebarOpen).toBe(false);
    });
  });

  // ── Navigation ───────────────────────────────────────────────────────────

  describe('Navigation', () => {
    it('highlights the correct nav item based on active route', () => {
      renderSidebar(['/skills']);

      const skillsBtn = screen.getByText('技能').closest('button')!;
      expect(skillsBtn.className).toContain('text-blue-700');

      const chatBtn = screen.getByText('会话').closest('button')!;
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

    it('clicking 会话 navigates to /chat', async () => {
      const user = userEvent.setup();
      renderSidebar();

      await user.click(screen.getByText('会话', { exact: true }));

      expect(screen.getByTestId('location-display').textContent).toBe('/chat');
    });
  });
});
