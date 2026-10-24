import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import AgentTasksPanel from '../AgentTasksPanel';
import DelegateActivity from '../DelegateActivity';
import { mockTaskCancelCalls, resetTaskMocks } from '@/test/mocks/handlers';
import { useAppStore } from '@/lib/store';

/** 会话后台任务面板（ADR-026：活跃在上、完成沉底、可展开、可取消）。 */

beforeEach(() => {
  resetTaskMocks();
});

describe('AgentTasksPanel', () => {
  it('renders nothing without a session (new chat)', () => {
    const { container } = render(<AgentTasksPanel sessionId={null} />);
    expect(container).toBeEmptyDOMElement();
  });

  it('shows session tasks with kind badges and cancel buttons', async () => {
    render(<AgentTasksPanel sessionId="session-1" />);

    // mockTasks: task-1(delegate running) + task-cmd-1(command running) @ session-1；
    // task-2 completed @ session-2、task-3 failed 无会话归属（不显示）
    await waitFor(() => {
      expect(screen.getByText(/运行中 2/)).toBeInTheDocument();
    });
    expect(screen.getByText(/写入文件/)).toBeInTheDocument();
    expect(screen.queryByText(/搜索代码库/)).not.toBeInTheDocument();
    // 类型徽标：委托 / 终端（后台终端命令对用户可见——此前只有 agent 能看到）
    expect(screen.getByText('委托')).toBeInTheDocument();
    expect(screen.getByText('终端')).toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: /取消后台任务/ })).toHaveLength(2);
  });

  it('renders nothing when the session has no tasks', async () => {
    const { container } = render(<AgentTasksPanel sessionId="session-9" />);
    // 轮询发出但过滤后为空 → 整条不渲染
    await waitFor(() => {
      expect(container).toBeEmptyDOMElement();
    });
  });

  it('keeps terminal tasks visible with status labels (done sink)', async () => {
    // 覆盖：session-1 只有终态任务 → 面板仍展示（已完成/已取消样式）
    const { mockTasks } = await import('@/test/mocks/handlers');
    const { http, HttpResponse } = await import('msw');
    const { server } = await import('@/test/mocks/server');
    const finished = [
      { ...mockTasks[0], status: 'completed' as const, completed_at: 1754399900000 },
      { ...mockTasks[1], status: 'cancelled' as const, completed_at: 1754399900001 },
    ];
    server.use(http.get('*/api/v1/tasks', () => HttpResponse.json(finished)));
    render(<AgentTasksPanel sessionId="session-1" />);
    await waitFor(() => {
      expect(screen.getByText(/已结束 2/)).toBeInTheDocument();
    });
    expect(screen.getByText('已完成')).toBeInTheDocument();
    expect(screen.getByText('已取消')).toBeInTheDocument();
    // 终态无取消按钮
    expect(screen.queryByRole('button', { name: /取消后台任务/ })).not.toBeInTheDocument();
  });

  it('shows failure reason for failed command tasks', async () => {
    const { mockTasks } = await import('@/test/mocks/handlers');
    const { http, HttpResponse } = await import('msw');
    const { server } = await import('@/test/mocks/server');
    const failed = [
      {
        ...mockTasks[1],
        status: 'failed' as const,
        error: '退出码 1',
        completed_at: 1754399900002,
      },
    ];
    server.use(http.get('*/api/v1/tasks', () => HttpResponse.json(failed)));
    render(<AgentTasksPanel sessionId="session-1" />);
    await waitFor(() => {
      expect(screen.getByText('失败（退出码 1）')).toBeInTheDocument();
    });
  });

  it('cancels a running task via POST /tasks/{id}/cancel', async () => {
    const user = userEvent.setup();
    render(<AgentTasksPanel sessionId="session-1" />);
    await waitFor(() => {
      expect(screen.getAllByRole('button', { name: /取消后台任务/ })[0]).toBeInTheDocument();
    });

    await user.click(screen.getAllByRole('button', { name: /取消后台任务/ })[0]);
    await waitFor(() => {
      expect(mockTaskCancelCalls).toEqual([{ taskId: 'task-1' }]);
    });
  });

  /**
   * 回归（zustand 5 升级引入，1.0.1 修复）：选择器返回值必须**引用稳定**。
   *
   * 旧实现 `useAppStore((s) => s.sessionMessages[taskId] ?? [])` 在 key 不存在时
   * 每次调用都返回新数组 → React 的 `useSyncExternalStore` 每次渲染都判定
   * 「store 变了」→ 无限重渲染 → `Maximum update depth exceeded`（React #185）
   * → 整页被 ErrorBoundary 兜住。
   *
   * 触发条件很日常：点开一个**尚未收到快照**的委托任务（子会话消息还没进 store）。
   *
   * 判别力：把选择器改回 `?? []`，本用例会在渲染期抛 #185 而失败。
   */
  it('renders DelegateActivity with an empty store without infinite re-render (React #185)', () => {
    useAppStore.setState({ sessionMessages: {} });
    // 若有 bug，render 本身会在渲染期抛 #185（Maximum update depth）→ 用例直接失败。
    const r = render(<DelegateActivity taskId="bt_not_in_store" status="completed" />);
    expect(screen.getByText('无过程记录')).toBeInTheDocument();
    r.unmount();
  });
});
