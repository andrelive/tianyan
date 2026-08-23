/** ToolsPanel 测试（F5）：工具列表渲染 / 错误态 / 空态。 */
import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { server } from '@/test/mocks/server';
import { http, HttpResponse } from 'msw';
import ToolsPanel from '../ToolsPanel';

const API_BASE = '/api/v1';
const TOOLS_FIXTURE = {
  tools: [
    { name: 'read_file', description: '读取文件', parameters: {} },
    { name: 'grep', description: '搜索内容', parameters: {} },
  ],
  total: 2,
};

function renderPanel() {
  return render(<ToolsPanel />);
}

beforeEach(() => {
  server.use(http.get(`${API_BASE}/tools`, () => HttpResponse.json(TOOLS_FIXTURE)));
});

describe('ToolsPanel', () => {
  it('renders the tool list from the API', async () => {
    renderPanel();
    await waitFor(() => {
      expect(screen.getByText('read_file')).toBeInTheDocument();
    });
    expect(screen.getByText('grep')).toBeInTheDocument();
    expect(screen.getByText(/2 个/)).toBeInTheDocument();
  });

  it('selects a tool and shows its description in the detail pane', async () => {
    const user = userEvent.setup();
    renderPanel();
    await user.click(await screen.findByText('read_file'));
    // 详情窗（列表行也含描述文本 → 用仅存在于详情窗的 Schema 头断言）
    expect(screen.getByText('参数 Schema')).toBeInTheDocument();
    expect(screen.getAllByText('读取文件').length).toBeGreaterThanOrEqual(2);
  });

  it('shows an error banner when the tools endpoint fails', async () => {
    server.use(
      http.get(`${API_BASE}/tools`, () => HttpResponse.json({ message: 'boom' }, { status: 500 })),
    );
    renderPanel();
    await waitFor(() => {
      // 500 时 ApiError 携带服务端消息（非 fallback）
      expect(screen.getByRole('alert')).toHaveTextContent(/boom|加载工具失败/);
    });
  });

  it('shows the empty state when no tools are returned', async () => {
    server.use(http.get(`${API_BASE}/tools`, () => HttpResponse.json({ tools: [], total: 0 })));
    renderPanel();
    await waitFor(() => {
      expect(screen.getByText('暂无可用工具')).toBeInTheDocument();
    });
  });
});
