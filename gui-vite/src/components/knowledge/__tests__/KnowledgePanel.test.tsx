import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { server } from '@/test/mocks/server';
import { http, HttpResponse } from 'msw';
import {
  mockKnowledgeDeleteCalls,
  mockKnowledgeSearchCalls,
  mockKnowledgeSuggestionsCalls,
  resetKnowledgeEntryMocks,
} from '@/test/mocks/handlers';
import KnowledgePanel from '../KnowledgePanel';

const API_BASE = '/api/v1';

function renderKnowledgePanel() {
  return render(
    <MemoryRouter>
      <KnowledgePanel />
    </MemoryRouter>,
  );
}

async function openBrowseTab(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole('tab', { name: /浏览/ }));
  await waitFor(() => {
    expect(screen.getByText('architecture')).toBeInTheDocument();
  });
}

beforeEach(() => {
  resetKnowledgeEntryMocks();
});

describe('KnowledgePanel', () => {
  it('renders knowledge entries in the browse tab', async () => {
    const user = userEvent.setup();
    renderKnowledgePanel();

    await openBrowseTab(user);

    expect(screen.getByText('architecture')).toBeInTheDocument();
    expect(screen.getByText('dual-layer-index')).toBeInTheDocument();
    expect(screen.getByText('ownership')).toBeInTheDocument();
    // 每条目都有删除按钮
    expect(screen.getByRole('button', { name: '删除 architecture' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '删除 ownership' })).toBeInTheDocument();
  });

  it('deletes an entry via ConfirmDialog, calls the API and refreshes the list', async () => {
    const user = userEvent.setup();
    renderKnowledgePanel();
    await openBrowseTab(user);

    // 删除按钮打开统一确认对话框（ConfirmDialog 原语）；目录条目文案说明递归删除子条目
    await user.click(screen.getByRole('button', { name: '删除 architecture' }));
    expect(screen.getByText(/将被永久删除（含全部子条目）/)).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '删除' }));

    // 调用了 delete API，uri 正确
    await waitFor(() => {
      expect(mockKnowledgeDeleteCalls).toContainEqual({
        uri: 'tianyan://knowledge/architecture',
      });
    });
    expect(mockKnowledgeDeleteCalls).toHaveLength(1);

    // 成功后刷新列表，被删条目消失、其余保留
    await waitFor(() => {
      expect(screen.queryByText('architecture')).not.toBeInTheDocument();
    });
    expect(screen.getByText('dual-layer-index')).toBeInTheDocument();
    expect(screen.getByText('ownership')).toBeInTheDocument();
  });

  it('does not call the delete API when confirmation is cancelled', async () => {
    const user = userEvent.setup();
    renderKnowledgePanel();
    await openBrowseTab(user);

    await user.click(screen.getByRole('button', { name: '删除 ownership' }));
    expect(screen.getByText(/将被永久删除/)).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '取消' }));

    // 未调用 API，对话框关闭，条目仍在
    expect(mockKnowledgeDeleteCalls).toHaveLength(0);
    expect(screen.queryByText(/将被永久删除/)).not.toBeInTheDocument();
    expect(screen.getByText('ownership')).toBeInTheDocument();
  });

  it('shows an error alert when the delete request fails', async () => {
    const user = userEvent.setup();
    server.use(
      http.post(`${API_BASE}/knowledge/entries/delete`, () => {
        return new HttpResponse(null, { status: 500 });
      }),
    );

    renderKnowledgePanel();
    await openBrowseTab(user);

    await user.click(screen.getByRole('button', { name: '删除 ownership' }));
    await user.click(screen.getByRole('button', { name: '删除' }));

    await waitFor(() => {
      expect(screen.getByRole('alert')).toBeInTheDocument();
    });
    expect(screen.getByRole('alert').textContent).toContain('HTTP 500');
    // 删除失败后条目保留
    expect(screen.getByText('ownership')).toBeInTheDocument();
  });

  it('does not call the delete API when row click browses the entry', async () => {
    const user = userEvent.setup();
    server.use(
      http.get(`${API_BASE}/knowledge/entries/read`, () => {
        return HttpResponse.json({
          uri: 'tianyan://knowledge/architecture/dual-layer-index',
          level: 'detail',
          content: 'mock content',
        });
      }),
    );
    renderKnowledgePanel();
    await openBrowseTab(user);

    // 点击条目行本身（非删除按钮）不会打开确认对话框，也不触发删除
    await user.click(screen.getByRole('button', { name: 'dual-layer-index' }));
    expect(screen.queryByText(/将被永久删除/)).not.toBeInTheDocument();
    expect(mockKnowledgeDeleteCalls).toHaveLength(0);
    expect(screen.getByText('mock content')).toBeInTheDocument();
  });

  it('shows an error alert when the ingest response reports per-file failure', async () => {
    server.use(
      http.post(`${API_BASE}/knowledge/ingest`, () =>
        HttpResponse.json({
          success: false,
          job_id: 'mock-ingest-job',
          message: '处理了 1 个文件 (共 10 字节)',
          files: [{ filename: 'broken.txt', status: 'failed', error: '解析失败：格式不支持' }],
        }),
      ),
    );
    const user = userEvent.setup();
    renderKnowledgePanel();

    await user.click(screen.getByRole('tab', { name: /导入/ }));
    const input = document.querySelector('input[type="file"]') as HTMLInputElement;
    await user.upload(input, new File(['内容'], 'broken.txt', { type: 'text/plain' }));
    await user.click(screen.getByRole('button', { name: /开始导入/ }));

    // 后端 HTTP 200 但文件级失败：UI 必须显示错误而非"导入成功"
    await waitFor(() => {
      expect(screen.getByRole('alert')).toHaveTextContent('解析失败');
    });
    expect(screen.queryByText('文件导入成功！')).not.toBeInTheDocument();
  });

  it('shows the success banner when all files complete (backend status "completed")', async () => {
    const user = userEvent.setup();
    renderKnowledgePanel();

    await user.click(screen.getByRole('tab', { name: /导入/ }));
    const input = document.querySelector('input[type="file"]') as HTMLInputElement;
    await user.upload(input, new File(['内容'], 'ok.txt', { type: 'text/plain' }));
    await user.click(screen.getByRole('button', { name: /开始导入/ }));

    await waitFor(() => {
      expect(screen.getByText('文件导入成功！')).toBeInTheDocument();
    });
  });

  it('reads content of a directory-flagged entry when it carries content levels (VFS doc nodes)', async () => {
    // VFS 中已 ingest 的文档以目录节点形态列出（is_directory=true 但携带 L0/L1/L2 内容），
    // 点击应读取内容而非导航进空目录（前后端对齐回归测试）。
    server.use(
      http.get(`${API_BASE}/knowledge/entries`, () =>
        HttpResponse.json({
          entries: [
            {
              uri: 'tianyan://knowledge/technical/doc123',
              name: 'doc123',
              is_directory: true,
              has_abstract: true,
              has_overview: true,
              has_detail: true,
            },
          ],
        }),
      ),
    );
    const user = userEvent.setup();
    renderKnowledgePanel();

    await user.click(screen.getByRole('tab', { name: /浏览/ }));
    await waitFor(() => {
      expect(screen.getByText('doc123')).toBeInTheDocument();
    });
    await user.click(screen.getByText('doc123'));

    // 内容应被读取并渲染（而非导航到子目录）：面包屑不出现 = 未发生导航
    await waitFor(() => {
      expect(screen.getByText('mock 读取内容')).toBeInTheDocument();
    });
    expect(screen.queryByRole('button', { name: '知识库' })).not.toBeInTheDocument();
  });

  it('shows suggestion chips after typing a partial query', async () => {
    const user = userEvent.setup();
    renderKnowledgePanel();

    await user.type(screen.getByLabelText('搜索知识库'), '架');

    // 300ms 防抖后拉取建议，chips 渲染
    await waitFor(() => {
      expect(screen.getByRole('button', { name: '架构' })).toBeInTheDocument();
    });
    expect(screen.getByRole('button', { name: 'VFS' })).toBeInTheDocument();
    expect(mockKnowledgeSuggestionsCalls).toContainEqual({ q: '架' });
  });

  it('fills the search box and fires a search request when a suggestion chip is clicked', async () => {
    const user = userEvent.setup();
    renderKnowledgePanel();

    await user.type(screen.getByLabelText('搜索知识库'), '架');

    await waitFor(() => {
      expect(screen.getByRole('button', { name: '架构' })).toBeInTheDocument();
    });

    await user.click(screen.getByRole('button', { name: '架构' }));

    // 搜索框值变为 chip 文本，且搜索请求以 chip 为查询条件发出
    expect(screen.getByLabelText('搜索知识库')).toHaveValue('架构');
    await waitFor(() => {
      expect(mockKnowledgeSearchCalls).toContainEqual({ q: '架构' });
    });
  });
});
