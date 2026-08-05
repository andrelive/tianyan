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

  it('renders memory entries from mock data', async () => {
    renderMemoryPanel();

    // 等待列表加载完成
    await waitFor(() => {
      expect(screen.getByText('rules')).toBeInTheDocument();
    });

    // 条目名称全部出现（目录 + 记忆条目）
    expect(screen.getByText('response_style')).toBeInTheDocument();
    expect(screen.getByText('user_name')).toBeInTheDocument();

    // 重要度展示
    expect(screen.getByText('90%')).toBeInTheDocument();
    expect(screen.getByText('60%')).toBeInTheDocument();

    // 标签展示
    expect(screen.getByText('偏好')).toBeInTheDocument();
    expect(screen.getByText('事实')).toBeInTheDocument();
  });

  it('shows content below when clicking an entry (detail fallback chain)', async () => {
    const user = userEvent.setup();
    renderMemoryPanel();

    await waitFor(() => {
      expect(screen.getByText('response_style')).toBeInTheDocument();
    });

    // 点击有 detail 内容的条目 → 显示 L2 详情
    await user.click(screen.getByText('response_style'));
    expect(screen.getByText('L2 详情')).toBeInTheDocument();
    expect(screen.getByText(/用户偏好简洁直接的回复风格/)).toBeInTheDocument();

    // 点击只有 abstract 的条目 → 回退到 L0 摘要
    await user.click(screen.getByText('user_name'));
    expect(screen.getByText('L0 摘要')).toBeInTheDocument();
    expect(screen.getByText(/用户昵称为「小天」/)).toBeInTheDocument();
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
    // 错误时不渲染条目列表
    expect(screen.queryByText('rules')).not.toBeInTheDocument();
  });
});
