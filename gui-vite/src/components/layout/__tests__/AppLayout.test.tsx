import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import AppLayout from '@/components/layout/AppLayout';

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState());
});

function renderAppLayoutWithChild(childContent: string = 'Main Content') {
  return render(
    <MemoryRouter initialEntries={['/']}>
      <Routes>
        <Route element={<AppLayout />}>
          <Route path="/" element={<div>{childContent}</div>} />
        </Route>
      </Routes>
    </MemoryRouter>,
  );
}

describe('AppLayout', () => {
  it('renders Sidebar with navigation items', () => {
    renderAppLayoutWithChild();
    // Sidebar renders "天演" header
    expect(screen.getByText('天演')).toBeInTheDocument();
    // Sidebar renders nav items
    expect(screen.getByText('会话')).toBeInTheDocument();
    expect(screen.getByText('技能')).toBeInTheDocument();
    expect(screen.getByText('知识')).toBeInTheDocument();
    expect(screen.getByText('设置')).toBeInTheDocument();
  });

  it('renders main content area', () => {
    const content = '这是 main 内容';
    renderAppLayoutWithChild(content);
    expect(screen.getByText(content)).toBeInTheDocument();
  });

  it('renders main element', () => {
    renderAppLayoutWithChild();
    // The layout has a main element
    const main = document.querySelector('main');
    expect(main).toBeInTheDocument();
    expect(main).toHaveClass('flex-1');
  });

  it('renders children via Outlet', () => {
    renderAppLayoutWithChild('子路由内容');
    // The child content passed via Outlet should be visible
    expect(screen.getByText('子路由内容')).toBeInTheDocument();
  });

  it('renders sidebar toggle button', () => {
    renderAppLayoutWithChild();
    // Sidebar has a toggle button when open (default isSidebarOpen=true)
    expect(screen.getByTitle('收起侧边栏')).toBeInTheDocument();
  });

  it('renders collapsed sidebar when isSidebarOpen is false', () => {
    useAppStore.setState({ isSidebarOpen: false });
    renderAppLayoutWithChild();

    // In collapsed mode, the sidebar has an expand button
    expect(screen.getByTitle('展开侧边栏')).toBeInTheDocument();
    // Nav icon buttons should still be visible (icon-only mode)
    expect(screen.getByTitle('会话')).toBeInTheDocument();
  });
});
