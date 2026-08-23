import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { server } from '@/test/mocks/server';
import { http, HttpResponse } from 'msw';
import { useAppStore } from '@/lib/store';
import { mockMcpServers } from '@/test/mocks/handlers';
import McpTab from '../tabs/McpTab';

const API_BASE = '/api/v1';

function renderMcpTab() {
  return render(
    <MemoryRouter>
      <McpTab />
    </MemoryRouter>,
  );
}

async function waitForServers() {
  await waitFor(() => {
    expect(screen.getByText('filesystem-server')).toBeInTheDocument();
  });
}

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState());
});

describe('McpTab', () => {
  it('loads and renders the server list with toggle and test statuses', async () => {
    renderMcpTab();
    await waitForServers();

    expect(screen.getByText('code-search-server')).toBeInTheDocument();
    expect(screen.getByText('文件系统操作')).toBeInTheDocument();
    // 启用态：filesystem-server 的开关已勾选
    const toggles = screen.getAllByRole('checkbox');
    expect(toggles[0]).toBeChecked();
    expect(toggles[1]).not.toBeChecked();
  });

  it('toggles a server optimistically and persists via API', async () => {
    const user = userEvent.setup();
    renderMcpTab();
    await waitForServers();

    await user.click(screen.getAllByRole('checkbox')[1]);
    await waitFor(() => {
      expect(screen.getAllByRole('checkbox')[1]).toBeChecked();
    });
  });

  it('tests connection and shows success with tool count', async () => {
    const user = userEvent.setup();
    renderMcpTab();
    await waitForServers();

    await user.click(screen.getAllByTitle('测试连接')[0]);
    await waitFor(() => {
      expect(screen.getByText('12 个工具可用')).toBeInTheDocument();
    });
  });

  it('removes a server through the ConfirmDialog flow (D4 原语)', async () => {
    const user = userEvent.setup();
    renderMcpTab();
    await waitForServers();

    await user.click(screen.getAllByTitle('移除服务器')[0]);
    // ConfirmDialog 出现
    expect(screen.getByText(/确定要移除 "filesystem-server" 吗/)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '移除' }));

    await waitFor(() => {
      expect(screen.queryByText('filesystem-server')).not.toBeInTheDocument();
    });
    expect(screen.getByText('code-search-server')).toBeInTheDocument();
  });

  it('does not call the API when the remove confirm is cancelled', async () => {
    const user = userEvent.setup();
    renderMcpTab();
    await waitForServers();

    await user.click(screen.getAllByTitle('移除服务器')[1]);
    await user.click(screen.getByRole('button', { name: '取消' }));

    expect(screen.getByText('code-search-server')).toBeInTheDocument();
  });

  it('adds a server via the form and shows it in the list', async () => {
    const user = userEvent.setup();
    renderMcpTab();
    await waitForServers();

    await user.click(screen.getByRole('button', { name: '添加服务器' }));
    await user.type(screen.getByPlaceholderText('例如: filesystem-server'), 'git-server');
    await user.type(screen.getByPlaceholderText('例如: npx'), 'npx');
    await user.click(screen.getByRole('button', { name: '保存' }));

    await waitFor(() => {
      expect(screen.getByText('git-server')).toBeInTheDocument();
    });
  });

  it('shows the load error state with a working retry button', async () => {
    const user = userEvent.setup();
    server.use(
      http.get(`${API_BASE}/config/mcp/servers`, () => new HttpResponse(null, { status: 500 })),
    );
    renderMcpTab();

    await waitFor(() => {
      expect(screen.getByRole('button', { name: '重试' })).toBeInTheDocument();
    });

    // 恢复后点击重试 → 列表加载
    server.use(http.get(`${API_BASE}/config/mcp/servers`, () => HttpResponse.json(mockMcpServers)));
    await user.click(screen.getByRole('button', { name: '重试' }));
    await waitFor(() => {
      expect(screen.getByText('filesystem-server')).toBeInTheDocument();
    });
  });
});
