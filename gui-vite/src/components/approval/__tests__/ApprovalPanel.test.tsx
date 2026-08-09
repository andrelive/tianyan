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

  it('shows command text and approves with edited command via inline editor', async () => {
    const user = userEvent.setup();
    let responded = false;
    let respondBody: unknown = null;

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
      expect(screen.getByText('执行命令 rm -rf /tmp/cache')).toBeInTheDocument();
    });

    // execute_command 类请求展示命令文本（mono block）
    expect(screen.getByText('rm -rf /tmp/cache')).toBeInTheDocument();
    // 非命令类请求（写入文件）不显示"编辑后批准"
    expect(
      screen.queryByRole('button', { name: /编辑后批准 写入文件/ }),
    ).not.toBeInTheDocument();

    // 打开内联编辑器 → 改写命令 → 提交编辑并批准
    await user.click(screen.getByRole('button', { name: /编辑后批准 执行命令/ }));
    const textarea = screen.getByRole('textbox', { name: '编辑后的命令' });
    expect(textarea).toHaveValue('rm -rf /tmp/cache');
    await user.clear(textarea);
    await user.type(textarea, 'rm -rf /tmp/cache --keep-log');

    await user.click(screen.getByRole('button', { name: '提交编辑并批准' }));

    await waitFor(() => {
      expect(respondBody).not.toBeNull();
    });
    expect(respondBody).toEqual({
      request_id: 'req-2',
      decision: 'approve',
      edited_command: 'rm -rf /tmp/cache --keep-log',
    });

    // 成功后刷新 → 空 pending 提示
    await waitFor(() => {
      expect(screen.getByText('暂无待处理审批')).toBeInTheDocument();
    });
  });

  it('cancels inline editing without responding', async () => {
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

    await user.click(screen.getByRole('button', { name: /编辑后批准 执行命令/ }));
    expect(screen.getByRole('textbox', { name: '编辑后的命令' })).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '取消' }));

    // 取消后编辑器关闭，未发起响应
    expect(screen.queryByRole('textbox', { name: '编辑后的命令' })).not.toBeInTheDocument();
    expect(respondSpy).not.toHaveBeenCalled();
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
