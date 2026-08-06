import { describe, it, expect, beforeAll, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { server } from '@/test/mocks/server';
import { http, HttpResponse } from 'msw';
import {
  resetWorkspaceMocks,
  mockWorkspaceDiffCalls,
  mockWorkspaceReadCalls,
} from '@/test/mocks/handlers';
import WorkspacePanel from '../WorkspacePanel';

const API_BASE = '/api/v1';

// jsdom 未实现 Range.getClientRects，CodeMirror 度量行高时会抛错（被 CM 内部捕获，
// 但会刷 stderr）。补齐最小实现以保持测试输出干净。
beforeAll(() => {
  if (typeof Range !== 'undefined' && typeof Range.prototype.getClientRects !== 'function') {
    Range.prototype.getClientRects = function () {
      return [new DOMRect(0, 0, 0, 0)] as unknown as DOMRectList;
    };
  }
});

function renderPanel() {
  return render(
    <MemoryRouter>
      <WorkspacePanel />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  resetWorkspaceMocks();
});

describe('WorkspacePanel', () => {
  it('renders the workspace file tree from the API', async () => {
    renderPanel();

    expect(screen.getByText('工作区')).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.getByText('Cargo.toml')).toBeInTheDocument();
    });
    expect(screen.getByText('src')).toBeInTheDocument();
    expect(screen.getByText('main.rs')).toBeInTheDocument();
  });

  it('loads file content into the viewer when a file is clicked', async () => {
    const user = userEvent.setup();
    renderPanel();

    await waitFor(() => {
      expect(screen.getByText('main.rs')).toBeInTheDocument();
    });
    await user.click(screen.getByText('main.rs'));

    // hashline 前缀默认剥离，只显示干净行
    // （CM6 语法高亮会把行内文本拆成多个 span，用 textContent 匹配 .cm-line）
    await waitFor(() => {
      expect(
        screen.getByText(
          (_, element) =>
            (element?.classList.contains('cm-line') ?? false) &&
            element?.textContent === 'fn main() {',
        ),
      ).toBeInTheDocument();
    });
    expect(screen.queryByText(/1#3f/)).not.toBeInTheDocument();
    expect(mockWorkspaceReadCalls).toContainEqual({ path: 'main.rs', limit: 2000 });
  });

  it('shows a binary notice when a binary file is selected', async () => {
    const user = userEvent.setup();
    renderPanel();

    await waitFor(() => {
      expect(screen.getByText('data.bin')).toBeInTheDocument();
    });
    await user.click(screen.getByText('data.bin'));

    await waitFor(() => {
      expect(screen.getByText(/二进制文件/)).toBeInTheDocument();
    });
    expect(screen.getByText(/binary preview/)).toBeInTheDocument();
  });

  it('shows an error message when the read API fails', async () => {
    const user = userEvent.setup();
    server.use(
      http.get(`${API_BASE}/workspace/read`, () => {
        return new HttpResponse(null, { status: 500 });
      }),
    );
    renderPanel();

    await waitFor(() => {
      expect(screen.getByText('Cargo.toml')).toBeInTheDocument();
    });
    await user.click(screen.getByText('Cargo.toml'));

    await waitFor(() => {
      expect(screen.getByRole('alert')).toBeInTheDocument();
    });
    expect(screen.getByRole('alert').textContent).toContain('HTTP 500');
  });

  it('loads a snapshot diff and renders +/- lines in the diff panel', async () => {
    const user = userEvent.setup();
    renderPanel();

    await waitFor(() => {
      expect(screen.getByText('main.rs')).toBeInTheDocument();
    });
    await user.click(screen.getByText('main.rs'));

    // diff 面板默认隐藏，先展开
    await user.click(screen.getByRole('button', { name: '展开 diff 面板' }));

    // 会话 ID 为必填（后端快照模式缺 session_id 返回 400）
    const sessionInput = screen.getByLabelText('会话 ID');
    await user.clear(sessionInput);
    await user.type(sessionInput, 'session-1');

    const indexInput = screen.getByLabelText('快照索引');
    await user.clear(indexInput);
    await user.type(indexInput, '2');
    await user.click(screen.getByRole('button', { name: '加载 diff' }));

    // unified diff 的 +/- 行按行渲染
    await waitFor(() => {
      expect(screen.getByText('+fn main() {')).toBeInTheDocument();
    });
    expect(screen.getByText('-fn main() {')).toBeInTheDocument();
    expect(mockWorkspaceDiffCalls).toContainEqual({
      path: 'main.rs',
      sessionId: 'session-1',
      index: 2,
    });
  });

  it('shows a validation error and skips the API when session id is empty', async () => {
    const user = userEvent.setup();
    renderPanel();

    await waitFor(() => {
      expect(screen.getByText('main.rs')).toBeInTheDocument();
    });
    await user.click(screen.getByText('main.rs'));
    await user.click(screen.getByRole('button', { name: '展开 diff 面板' }));

    // store 中无当前会话 → 会话 ID 为空，直接点击应显示校验错误且不发请求
    const sessionInput = screen.getByLabelText('会话 ID');
    await user.clear(sessionInput);
    await user.click(screen.getByRole('button', { name: '加载 diff' }));

    await waitFor(() => {
      expect(screen.getByText('请填写会话 ID')).toBeInTheDocument();
    });
    expect(mockWorkspaceDiffCalls).toHaveLength(0);
  });
});
