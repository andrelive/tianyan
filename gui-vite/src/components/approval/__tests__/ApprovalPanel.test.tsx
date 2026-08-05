import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import { server } from '@/test/mocks/server';
import { http, HttpResponse } from 'msw';
import { mockApprovalStatus } from '@/test/mocks/handlers';
import ApprovalPanel from '../ApprovalPanel';

function renderApprovalPanel() {
  return render(
    <MemoryRouter>
      <ApprovalPanel />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState());
});

describe('ApprovalPanel', () => {
  it('renders header title', async () => {
    renderApprovalPanel();
    expect(screen.getByText('审批')).toBeInTheDocument();
  });

  it('renders pending approvals, audit records and config summary from mock data', async () => {
    renderApprovalPanel();

    // 待处理审批列表
    await waitFor(() => {
      expect(screen.getByText('写入文件 /home/user/data.txt')).toBeInTheDocument();
    });
    expect(screen.getByText('执行命令 rm -rf /tmp/cache')).toBeInTheDocument();

    // 风险等级 badge（Critical → 危险，High → 高）
    expect(screen.getByText('危险')).toBeInTheDocument();
    expect(screen.getByText('高')).toBeInTheDocument();

    // 最近审计记录 + 决策标签（后端 Approve → 批准）+ 审批人
    expect(screen.getByText('写入文件 /home/user/old.txt')).toBeInTheDocument();
    expect(screen.getByText('user')).toBeInTheDocument();

    // 审批配置摘要
    expect(screen.getByText('等待人工响应（降级链路）')).toBeInTheDocument();
    expect(screen.getByText('自动批准')).toBeInTheDocument();
    expect(screen.getByText('无人值守模式')).toBeInTheDocument();
    expect(screen.getByText('已确认操作数')).toBeInTheDocument();
    expect(screen.getByText('300 秒')).toBeInTheDocument();
  });

  it('approves a pending request, calls respond API and refreshes the list', async () => {
    const user = userEvent.setup();
    let responded = false;
    let respondBody: unknown = null;

    // respond 成功后，status 返回空 pending（模拟后端移除该请求）
    server.use(
      http.post('/api/v1/approval/respond', async ({ request }) => {
        responded = true;
        respondBody = await request.json();
        return HttpResponse.json({ ok: true });
      }),
      http.get('/api/v1/approval/status', () => {
        return HttpResponse.json(
          responded ? { ...mockApprovalStatus, pending_approvals: [] } : mockApprovalStatus,
        );
      }),
    );

    renderApprovalPanel();
    await waitFor(() => {
      expect(screen.getByText('写入文件 /home/user/data.txt')).toBeInTheDocument();
    });

    await user.click(screen.getByRole('button', { name: '批准 写入文件 /home/user/data.txt' }));

    // respond 收到 approve 决策
    await waitFor(() => {
      expect(responded).toBe(true);
    });
    expect(respondBody).toEqual({ request_id: 'req-1', decision: 'approve' });

    // 成功后立即刷新 → 空 pending 提示
    await waitFor(() => {
      expect(screen.getByText('暂无待处理审批')).toBeInTheDocument();
    });
    expect(screen.queryByText('写入文件 /home/user/data.txt')).not.toBeInTheDocument();
  });

  it('denies a pending request with deny decision', async () => {
    const user = userEvent.setup();
    const respondSpy = vi.fn();

    server.use(
      http.post('/api/v1/approval/respond', async ({ request }) => {
        respondSpy(await request.json());
        return HttpResponse.json({ ok: true });
      }),
    );

    renderApprovalPanel();
    await waitFor(() => {
      expect(screen.getByText('执行命令 rm -rf /tmp/cache')).toBeInTheDocument();
    });

    await user.click(screen.getByRole('button', { name: '拒绝 执行命令 rm -rf /tmp/cache' }));

    await waitFor(() => {
      expect(respondSpy).toHaveBeenCalledTimes(1);
    });
    expect(respondSpy).toHaveBeenCalledWith({ request_id: 'req-2', decision: 'deny' });
  });

  it('shows hint when wait_for_approval is disabled and no pending approvals', async () => {
    server.use(
      http.get('/api/v1/approval/status', () => {
        return HttpResponse.json({
          ...mockApprovalStatus,
          config: { ...mockApprovalStatus.config, wait_for_approval: false },
          pending_approvals: [],
        });
      }),
    );

    renderApprovalPanel();

    await waitFor(() => {
      expect(screen.getByText('暂无待处理审批')).toBeInTheDocument();
    });
    expect(
      screen.getByText(
        '审批等待模式未开启（配置 security.wait_for_approval=false 时危险操作走对话确认链路）',
      ),
    ).toBeInTheDocument();
  });

  it('shows error state with retry button when the request fails', async () => {
    server.use(
      http.get('/api/v1/approval/status', () => {
        return new HttpResponse(null, { status: 500 });
      }),
    );

    renderApprovalPanel();

    await waitFor(() => {
      expect(screen.getByRole('alert')).toBeInTheDocument();
    });
    expect(screen.getByRole('alert').textContent).toContain('HTTP 500');
    expect(screen.getByRole('button', { name: '重试' })).toBeInTheDocument();
    // 错误时不渲染审批列表
    expect(screen.queryByText('写入文件 /home/user/data.txt')).not.toBeInTheDocument();
  });
});
