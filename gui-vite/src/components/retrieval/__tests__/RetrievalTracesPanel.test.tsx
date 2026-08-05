import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import { server } from '@/test/mocks/server';
import { http, HttpResponse } from 'msw';
import RetrievalTracesPanel from '../RetrievalTracesPanel';

function renderTracesPanel() {
  return render(
    <MemoryRouter>
      <RetrievalTracesPanel />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState());
});

describe('RetrievalTracesPanel', () => {
  it('renders header title', async () => {
    renderTracesPanel();
    expect(screen.getByText('检索轨迹')).toBeInTheDocument();
  });

  it('renders traces from mock data', async () => {
    renderTracesPanel();

    // 等待列表加载完成
    await waitFor(() => {
      expect(screen.getByText('VFS 双层摘要索引架构')).toBeInTheDocument();
    });

    // 其余轨迹的 query 也出现
    expect(screen.getByText('Rust 内存安全模式')).toBeInTheDocument();
    expect(screen.getByText('SQLite 单连接限制')).toBeInTheDocument();

    // 列表项展示总耗时 + Token 数
    expect(screen.getByText(/1\.28s · 2430 Tokens/)).toBeInTheDocument();
    expect(screen.getByText(/860ms · 1540 Tokens/)).toBeInTheDocument();
  });

  it('expands detail when clicking a trace', async () => {
    const user = userEvent.setup();
    renderTracesPanel();

    await waitFor(() => {
      expect(screen.getByText('VFS 双层摘要索引架构')).toBeInTheDocument();
    });

    // 点击第一条轨迹 → 右侧展开详情
    await user.click(screen.getByText('VFS 双层摘要索引架构'));

    // 步骤时间线徽标（意图分析 / L1 搜索 / 内容加载）
    expect(screen.getByText('意图分析')).toBeInTheDocument();
    expect(screen.getByText('L0 搜索')).toBeInTheDocument();
    expect(screen.getByText('L1 搜索')).toBeInTheDocument();
    expect(screen.getByText('内容加载')).toBeInTheDocument();
    expect(screen.getByText('聚合')).toBeInTheDocument();

    // 分数：有值展示三位小数，null 展示 —
    expect(screen.getByText('分数 0.870')).toBeInTheDocument();
    expect(screen.getByText('分数 0.720')).toBeInTheDocument();
    expect(screen.getAllByText('分数 —').length).toBeGreaterThan(0);

    // 步骤目标 URI（步骤 target_uri 与结果列表均可能出现 → 用 getAllByText）
    expect(
      screen.getAllByText('tianyan://knowledge/architecture/dual-layer-index').length,
    ).toBeGreaterThan(0);
    expect(screen.getAllByText('tianyan://knowledge/architecture/vfs').length).toBeGreaterThan(0);

    // 总耗时 / 总 Token 统计
    expect(screen.getByText(/总耗时 1\.28s/)).toBeInTheDocument();
    expect(screen.getByText(/总 Token 2430/)).toBeInTheDocument();
  });

  it('shows error state when the request fails', async () => {
    // 覆盖 /retrieval/traces 返回 500
    server.use(
      http.get('/api/v1/retrieval/traces', () => {
        return new HttpResponse(null, { status: 500 });
      }),
    );

    renderTracesPanel();

    await waitFor(() => {
      expect(screen.getByRole('alert')).toBeInTheDocument();
    });
    expect(screen.getByRole('alert').textContent).toContain('HTTP 500');
    // 错误时不渲染轨迹列表
    expect(screen.queryByText('VFS 双层摘要索引架构')).not.toBeInTheDocument();
  });
});
