import { describe, it, expect, beforeEach } from 'vitest';
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
// Suite（图标 rail：恒 64px，hover 显示名字，无展开/收起）
// ---------------------------------------------------------------------------

describe('Sidebar', () => {
  beforeEach(() => {
    useAppStore.setState({
      sessions: [],
      currentSessionId: null,
      currentView: 'chat',
      messages: [],
      toasts: [],
    });
  });

  it('renders a 64px icon rail', () => {
    renderSidebar();
    const aside = screen.getByRole('navigation', { name: '导航' });
    expect(aside.className).toContain('w-[64px]');
  });

  it('shows the brand block and all nav icons with hover names (title)', () => {
    renderSidebar();
    expect(screen.getByTitle('天演')).toBeInTheDocument();
    expect(screen.getByTitle('会话')).toBeInTheDocument();
    expect(screen.getByTitle('技能')).toBeInTheDocument();
    expect(screen.getByTitle('知识')).toBeInTheDocument();
    expect(screen.getByTitle('记忆')).toBeInTheDocument();
    expect(screen.getByTitle('检索轨迹')).toBeInTheDocument();
    expect(screen.getByTitle('审批')).toBeInTheDocument();
    expect(screen.getByTitle('任务')).toBeInTheDocument();
    expect(screen.getByTitle('洞察')).toBeInTheDocument();
    expect(screen.getByTitle('设置')).toBeInTheDocument();
  });

  it('does not show the old 工作区 nav item or session list', () => {
    renderSidebar();
    expect(screen.queryByTitle('工作区')).not.toBeInTheDocument();
    expect(screen.queryByText('暂无会话')).not.toBeInTheDocument();
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

  it('highlights the correct nav item based on active route', () => {
    renderSidebar(['/skills']);

    const skillsBtn = screen.getByTitle('技能');
    expect(skillsBtn.className).toContain('text-blue-600');

    const chatBtn = screen.getByTitle('会话');
    expect(chatBtn.className).not.toContain('text-blue-600');
  });

  it('clicking 会话 navigates to /chat', async () => {
    const user = userEvent.setup();
    renderSidebar();

    await user.click(screen.getByTitle('会话'));

    expect(screen.getByTestId('location-display').textContent).toBe('/chat');
  });
});