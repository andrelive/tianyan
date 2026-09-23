import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import { server } from '@/test/mocks/server';
import { http, HttpResponse } from 'msw';
import MemoryPanel from '../MemoryPanel';

function renderMemoryPanel() {
  return render(
    <MemoryRouter>
      <MemoryPanel />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState());
});

describe('MemoryPanel', () => {
  it('renders header title', async () => {
    renderMemoryPanel();
    expect(screen.getByText('记忆')).toBeInTheDocument();
  });

  it('renders category tree with directory nodes and leaves', async () => {
    renderMemoryPanel();

    // 等待加载完成（目录节点出现）
    await waitFor(() => {
      expect(screen.getByText('facts')).toBeInTheDocument();
    });

    // 目录节点：preferences / facts / cases / failed_tasks（多级）
    expect(screen.getByText('preferences')).toBeInTheDocument();
    expect(screen.getByText('cases')).toBeInTheDocument();
    expect(screen.getByText('failed_tasks')).toBeInTheDocument();

    // 叶子显示条目名（路径信息由树结构表达）
    expect(screen.getByText('response_style')).toBeInTheDocument();
    expect(screen.getByText('user_name')).toBeInTheDocument();
    expect(screen.getByText('old_case')).toBeInTheDocument();

    // 重要度展示
    expect(screen.getByText('90%')).toBeInTheDocument();
    expect(screen.getByText('60%')).toBeInTheDocument();
    expect(screen.getByText('70%')).toBeInTheDocument();

    // 标签展示
    expect(screen.getByText('偏好')).toBeInTheDocument();
    expect(screen.getByText('事实')).toBeInTheDocument();
  });

  it('switches L0/L1/L2 levels (default = most detailed available)', async () => {
    const user = userEvent.setup();
    renderMemoryPanel();

    await waitFor(() => {
      expect(screen.getByText('response_style')).toBeInTheDocument();
    });

    // 点击三层齐全的条目 → 默认 L2 详情
    await user.click(screen.getByText('response_style'));
    expect(screen.getByRole('tab', { name: '正文' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByText(/技术问答时/)).toBeInTheDocument();

    // 切到 L0 摘要
    await user.click(screen.getByRole('tab', { name: '简介' }));
    expect(
      screen.getByText('用户偏好简洁直接的回复风格，代码示例优先使用 Rust。'),
    ).toBeInTheDocument();

    // 切到 L1 概览
    await user.click(screen.getByRole('tab', { name: '目录' }));
    expect(screen.getByText(/用户希望回复简洁直接/)).toBeInTheDocument();

    // 点击只有 abstract 的条目 → 回退 L0，L1/L2 禁用
    await user.click(screen.getByText('user_name'));
    expect(screen.getByRole('tab', { name: '简介' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByText(/用户昵称为「小天」/)).toBeInTheDocument();
    expect(screen.getByRole('tab', { name: '目录' })).toBeDisabled();
    expect(screen.getByRole('tab', { name: '正文' })).toBeDisabled();
  });

  it('renders structured index (L1) as readable list', async () => {
    // L1 = 目录（结构化 JSON）→ 渲染为可读列表（与后端 render_doc_index 对齐）；
    // 非目录（旧概览/全文直用）原样展示。
    const indexJson = JSON.stringify({
      kind: 'index',
      sections: [{ title: '安装', summary: '如何安装', start_line: 3, end_line: 10 }],
    });
    server.use(
      http.get('/api/v1/memory', () =>
        HttpResponse.json({
          memories: [
            {
              uri: 'tianyan://memory/facts/indexed',
              is_directory: false,
              name: 'indexed',
              relative_path: 'facts/indexed',
              metadata: {
                uri: {
                  uri: 'tianyan://memory/facts/indexed',
                  namespace: 'memory',
                  path: ['facts', 'indexed'],
                },
                is_directory: false,
                content_type: 'text/plain',
                category: 'fact',
                source: 'MemoryTask',
                original_name: 'indexed.md',
                file_size: 100,
                importance: 0.5,
                tags: [],
                created_at: '2026-07-22T11:00:00Z',
                updated_at: '2026-07-22T11:00:00Z',
                custom: {},
              },
              abstract: '目录条目简介',
              overview: indexJson,
              detail: '正文内容',
            },
          ],
          total: 1,
        }),
      ),
    );

    const user = userEvent.setup();
    renderMemoryPanel();

    await waitFor(() => {
      expect(screen.getByText('indexed')).toBeInTheDocument();
    });
    await user.click(screen.getByText('indexed'));
    // 默认层级 = 正文（detail）
    expect(screen.getByText('正文内容')).toBeInTheDocument();
    // 切到目录：JSON 渲染为可读列表（而非原始 JSON）
    await user.click(screen.getByRole('tab', { name: '目录' }));
    expect(screen.getByText('- 安装（行 3-10）：如何安装')).toBeInTheDocument();
    expect(screen.queryByText(/"sections"/)).not.toBeInTheDocument();
  });

  it('collapses and expands directory nodes', async () => {
    const user = userEvent.setup();
    renderMemoryPanel();

    await waitFor(() => {
      expect(screen.getByText('user_name')).toBeInTheDocument();
    });

    // 点击 facts 目录 → 折叠（其下叶子消失）
    await user.click(screen.getByText('facts'));
    expect(screen.queryByText('user_name')).not.toBeInTheDocument();

    // 再点击 → 展开恢复
    await user.click(screen.getByText('facts'));
    expect(screen.getByText('user_name')).toBeInTheDocument();
  });

  it('shows error state when the request fails', async () => {
    // 覆盖 /memory 返回 500
    server.use(
      http.get('/api/v1/memory', () => {
        return new HttpResponse(null, { status: 500 });
      }),
    );

    renderMemoryPanel();

    await waitFor(() => {
      expect(screen.getByRole('alert')).toBeInTheDocument();
    });
    expect(screen.getByRole('alert').textContent).toContain('HTTP 500');
    // 错误时不渲染树内容
    expect(screen.queryByText('facts')).not.toBeInTheDocument();
  });
});
