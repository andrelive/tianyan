import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { server } from '@/test/mocks/server';
import { http, HttpResponse } from 'msw';
import { mockKnowledgeDeleteCalls, resetKnowledgeEntryMocks } from '@/test/mocks/handlers';
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

  it('deletes an entry after inline confirmation, calls the API and refreshes the list', async () => {
    const user = userEvent.setup();
    renderKnowledgePanel();
    await openBrowseTab(user);

    // 目录条目删除确认文案说明递归删除子条目
    await user.click(screen.getByRole('button', { name: '删除 architecture' }));
    expect(screen.getByText('将同时删除其全部子条目')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '确认删除 architecture' }));

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
    expect(screen.getByRole('button', { name: '确认删除 ownership' })).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '取消' }));

    // 未调用 API，确认态消失，条目仍在
    expect(mockKnowledgeDeleteCalls).toHaveLength(0);
    expect(screen.queryByRole('button', { name: '确认删除 ownership' })).not.toBeInTheDocument();
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
    await user.click(screen.getByRole('button', { name: '确认删除 ownership' }));

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

    // 点击条目行本身（非删除按钮）不会进入确认态，也不触发删除
    await user.click(screen.getByRole('button', { name: 'dual-layer-index' }));
    expect(
      screen.queryByRole('button', { name: '确认删除 dual-layer-index' }),
    ).not.toBeInTheDocument();
    expect(mockKnowledgeDeleteCalls).toHaveLength(0);
    expect(screen.getByText('mock content')).toBeInTheDocument();
  });
});
