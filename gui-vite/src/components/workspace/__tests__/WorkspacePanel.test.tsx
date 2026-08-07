import { describe, it, expect, beforeAll, beforeEach } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { server } from '@/test/mocks/server';
import { http, HttpResponse } from 'msw';
import {
  resetWorkspaceMocks,
  mockWorkspaceDiffCalls,
  mockWorkspaceReadCalls,
  mockWorkspaceApplyPatchCalls,
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

  it('opens the save-confirm dialog, confirms, and submits the generated patch', async () => {
    const user = userEvent.setup();
    renderPanel();

    await waitFor(() => {
      expect(screen.getByText('main.rs')).toBeInTheDocument();
    });
    await user.click(screen.getByText('main.rs'));

    // 进入编辑模式并输入修改
    await waitFor(() => {
      expect(screen.getByRole('button', { name: /编辑/ })).toBeInTheDocument();
    });
    await user.click(screen.getByRole('button', { name: /编辑/ }));
    await waitFor(() => {
      expect(screen.getByRole('button', { name: '保存' })).toBeInTheDocument();
    });
    const cmContent = screen.getByText(
      (_, element) => element?.classList.contains('cm-content') ?? false,
    );
    await user.click(cmContent);
    await user.keyboard('x');
    await waitFor(() => {
      expect(screen.getByLabelText('未保存')).toBeInTheDocument();
    });

    // 点保存 → diff 确认弹窗出现（含统一 diff 视图与磁盘原文内容）
    await user.click(screen.getByRole('button', { name: '保存' }));
    const dialog = await screen.findByRole('dialog', { name: '保存确认' });
    expect(dialog).toBeInTheDocument();
    // unifiedMergeView 会把变更行以"删除块 + 当前行"各渲染一次
    expect(within(dialog).getAllByText(/fn main\(\) \{/).length).toBeGreaterThanOrEqual(1);

    // 确认保存 → apply-patch 收到包含标准 unified patch 头的调用 → 弹窗关闭
    await user.click(screen.getByRole('button', { name: '确认保存' }));
    await waitFor(() => {
      expect(mockWorkspaceApplyPatchCalls).toHaveLength(1);
    });
    const { patch } = mockWorkspaceApplyPatchCalls[0];
    expect(patch).toContain('--- a/main.rs');
    expect(patch).toContain('+++ b/main.rs');
    expect(patch).toContain('@@');
    await waitFor(() => {
      expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    });
  });

  it('shows the conflict message inside the dialog when apply-patch returns 409', async () => {
    const user = userEvent.setup();
    server.use(
      http.post(`${API_BASE}/workspace/apply-patch`, async ({ request }) => {
        const body = (await request.json()) as { patch?: string };
        mockWorkspaceApplyPatchCalls.push({ patch: body.patch ?? '' });
        return new HttpResponse(JSON.stringify({ message: 'conflict', code: 'conflict' }), {
          status: 409,
        });
      }),
    );
    renderPanel();

    await waitFor(() => {
      expect(screen.getByText('main.rs')).toBeInTheDocument();
    });
    await user.click(screen.getByText('main.rs'));
    await waitFor(() => {
      expect(screen.getByRole('button', { name: /编辑/ })).toBeInTheDocument();
    });
    await user.click(screen.getByRole('button', { name: /编辑/ }));
    await waitFor(() => {
      expect(screen.getByRole('button', { name: '保存' })).toBeInTheDocument();
    });
    const cmContent = screen.getByText(
      (_, element) => element?.classList.contains('cm-content') ?? false,
    );
    await user.click(cmContent);
    await user.keyboard('y');
    await waitFor(() => {
      expect(screen.getByLabelText('未保存')).toBeInTheDocument();
    });

    await user.click(screen.getByRole('button', { name: '保存' }));
    await screen.findByRole('dialog', { name: '保存确认' });
    await user.click(screen.getByRole('button', { name: '确认保存' }));

    // 409 → 弹窗内显示冲突提示，弹窗保持打开
    await waitFor(() => {
      expect(screen.getByRole('alert')).toBeInTheDocument();
    });
    expect(screen.getByRole('alert').textContent).toContain('文件已被修改，请重新加载');
    expect(screen.getByRole('dialog', { name: '保存确认' })).toBeInTheDocument();
    expect(mockWorkspaceApplyPatchCalls).toHaveLength(1);
  });
});
