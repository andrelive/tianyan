import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import { server } from '@/test/mocks/server';
import { http, HttpResponse } from 'msw';
import { resetTaskMocks, mockTaskCancelCalls } from '@/test/mocks/handlers';
import TasksPanel from '../TasksPanel';

function renderTasksPanel() {
  return render(
    <MemoryRouter>
      <TasksPanel />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState());
  resetTaskMocks();
});

describe('TasksPanel', () => {
  it('renders header title', async () => {
    renderTasksPanel();
    expect(screen.getByText('任务')).toBeInTheDocument();
  });

  it('renders tasks with status badges, session and timestamps from mock data', async () => {
    renderTasksPanel();

    // 任务描述
    await waitFor(() => {
      expect(screen.getByText('写入文件 /home/user/report.md')).toBeInTheDocument();
    });
    expect(screen.getByText('搜索代码库中的 TODO 标记')).toBeInTheDocument();
    expect(screen.getByText('解析大型文档并生成摘要')).toBeInTheDocument();

    // 状态徽标（running=运行中，completed=已完成，failed=失败）
    expect(screen.getByText('运行中')).toBeInTheDocument();
    expect(screen.getByText('已完成')).toBeInTheDocument();
    expect(screen.getByText('失败')).toBeInTheDocument();

    // 所属会话（null 显示 —）
    expect(screen.getByText('会话 session-1')).toBeInTheDocument();
    expect(screen.getByText('会话 —')).toBeInTheDocument();

    // 终态任务：结果摘要与错误
    expect(screen.getByText('找到 12 处 TODO 标记')).toBeInTheDocument();
    expect(screen.getByText('读取文件超时: 网络错误')).toBeInTheDocument();
  });

  it('shows cancel button only for running/pending tasks', async () => {
    renderTasksPanel();

    await waitFor(() => {
      expect(screen.getByText('写入文件 /home/user/report.md')).toBeInTheDocument();
    });

    // running 任务有取消按钮
    expect(
      screen.getByRole('button', { name: '取消任务 写入文件 /home/user/report.md' }),
    ).toBeInTheDocument();
    // 终态任务（completed/failed）无取消按钮
    expect(
      screen.queryByRole('button', { name: '取消任务 搜索代码库中的 TODO 标记' }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: '取消任务 解析大型文档并生成摘要' }),
    ).not.toBeInTheDocument();
  });

  it('cancels a running task, calls the API and refreshes the list', async () => {
    const user = userEvent.setup();
    renderTasksPanel();

    await waitFor(() => {
      expect(screen.getByText('写入文件 /home/user/report.md')).toBeInTheDocument();
    });

    await user.click(
      screen.getByRole('button', { name: '取消任务 写入文件 /home/user/report.md' }),
    );

    // cancel API 被调用
    await waitFor(() => {
      expect(mockTaskCancelCalls).toEqual([{ taskId: 'task-1' }]);
    });

    // 取消后刷新列表：状态变为已取消、取消按钮消失
    await waitFor(() => {
      expect(screen.getAllByText('已取消').length).toBeGreaterThan(0);
    });
    expect(
      screen.queryByRole('button', { name: '取消任务 写入文件 /home/user/report.md' }),
    ).not.toBeInTheDocument();
  });

  it('shows empty state when there are no tasks', async () => {
    server.use(
      http.get('/api/v1/tasks', () => {
        return HttpResponse.json([]);
      }),
    );

    renderTasksPanel();

    await waitFor(() => {
      expect(screen.getByText('暂无任务')).toBeInTheDocument();
    });
  });

  it('does not crash when the request fails (keeps old data)', async () => {
    server.use(
      http.get('/api/v1/tasks', () => {
        return new HttpResponse(null, { status: 500 });
      }),
    );

    renderTasksPanel();

    // 请求失败：面板不崩溃，静默显示空列表（无错误横幅、无 spin 死循环）
    await waitFor(() => {
      expect(screen.getByText('任务')).toBeInTheDocument();
    });
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });
});
