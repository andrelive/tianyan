import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import SessionTasksPanel from '../SessionTasksPanel';
import { mockTaskCancelCalls, resetTaskMocks } from '@/test/mocks/handlers';

/** 会话后台任务停靠条（会话绑定 + 只展示 pending/running + 可取消）。 */

beforeEach(() => {
  resetTaskMocks();
});

describe('SessionTasksPanel', () => {
  it('renders nothing without a session (new chat)', () => {
    const { container } = render(<SessionTasksPanel sessionId={null} />);
    expect(container).toBeEmptyDOMElement();
  });

  it('shows running tasks bound to the current session with cancel buttons', async () => {
    render(<SessionTasksPanel sessionId="session-1" />);

    // mockTasks: task-1 running @ session-1；task-2 completed @ session-2（不显示）
    await waitFor(() => {
      expect(screen.getByText(/后台任务 1 个运行中/)).toBeInTheDocument();
    });
    expect(screen.getByText(/写入文件/)).toBeInTheDocument();
    expect(screen.queryByText(/搜索代码库/)).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: /取消后台任务/ })).toBeInTheDocument();
  });

  it('renders nothing when the session has no running tasks', async () => {
    const { container } = render(<SessionTasksPanel sessionId="session-9" />);
    // 轮询发出但过滤后为空 → 整条不渲染
    await waitFor(() => {
      expect(container).toBeEmptyDOMElement();
    });
  });

  it('cancels a running task via POST /tasks/{id}/cancel', async () => {
    const user = userEvent.setup();
    render(<SessionTasksPanel sessionId="session-1" />);
    await waitFor(() => {
      expect(screen.getByRole('button', { name: /取消后台任务/ })).toBeInTheDocument();
    });

    await user.click(screen.getByRole('button', { name: /取消后台任务/ }));
    await waitFor(() => {
      expect(mockTaskCancelCalls).toEqual([{ taskId: 'task-1' }]);
    });
  });
});
