import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import { server } from '@/test/mocks/server';
import { http, HttpResponse } from 'msw';
import { mockSchedulerStatus, mockUsageStats } from '@/test/mocks/handlers';
import InsightsPanel from '../InsightsPanel';

function renderInsightsPanel() {
  return render(
    <MemoryRouter>
      <InsightsPanel />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState());
});

describe('InsightsPanel', () => {
  it('renders header title', async () => {
    renderInsightsPanel();
    expect(screen.getByText('洞察')).toBeInTheDocument();
  });

  it('renders scheduler task table from mock data', async () => {
    renderInsightsPanel();

    // 任务名
    await waitFor(() => {
      expect(screen.getByText('摘要生成')).toBeInTheDocument();
    });
    expect(screen.getByText('垃圾回收')).toBeInTheDocument();

    // 任务 ID + 优先级 badge + 间隔展示 + 累计执行次数（ADR-024 间隔制）
    expect(screen.getByText('summary_generation')).toBeInTheDocument();
    expect(screen.getAllByText('Medium').length).toBeGreaterThan(0);
    expect(screen.getByText('Low')).toBeInTheDocument();
    expect(screen.getAllByText('每 6 小时').length).toBe(2);
    expect(screen.getByText('12 次')).toBeInTheDocument();
    expect(screen.getByText('2 次')).toBeInTheDocument();

    // 运行中徽标
    expect(screen.getByText('运行中')).toBeInTheDocument();
  });

  it('renders usage stats cards from mock data', async () => {
    renderInsightsPanel();

    await waitFor(() => {
      expect(screen.getByText('124')).toBeInTheDocument();
    });
    expect(screen.getByText('7')).toBeInTheDocument();
    expect(screen.getByText('45')).toBeInTheDocument();
    expect(screen.getByText('230')).toBeInTheDocument();

    expect(screen.getByText('技能追踪数')).toBeInTheDocument();
    expect(screen.getByText('技能调用数')).toBeInTheDocument();
    expect(screen.getByText('工具追踪数')).toBeInTheDocument();
    expect(screen.getByText('工具调用数')).toBeInTheDocument();
    expect(screen.getByText('9')).toBeInTheDocument();
    expect(screen.getByText('856')).toBeInTheDocument();
    expect(screen.getByText('文档追踪数')).toBeInTheDocument();
    expect(screen.getByText('搜索次数')).toBeInTheDocument();
  });

  it('shows 从未执行 for tasks that never ran, and formatted elapsed time otherwise', async () => {
    // 覆盖：包含一个从未执行（last_run_ago_secs = null）的任务
    const { http, HttpResponse } = await import('msw');
    const { server } = await import('@/test/mocks/server');
    server.use(
      http.get('/api/v1/scheduler/status', () => {
        return HttpResponse.json({
          running: true,
          tasks: [
            {
              id: 'evolution',
              name: '自演化综述',
              priority: 'High',
              interval_secs: 86400,
              run_count: 0,
              last_run_ago_secs: null,
              next_due_in_secs: 0,
            },
            {
              id: 'summary_generation',
              name: '摘要生成',
              priority: 'Medium',
              interval_secs: 21600,
              run_count: 12,
              last_run_ago_secs: 183,
              next_due_in_secs: 21417,
            },
            {
              id: 'garbage_collection',
              name: '垃圾回收',
              priority: 'Low',
              interval_secs: 21600,
              run_count: 2,
              last_run_ago_secs: 3600,
              next_due_in_secs: 18000,
            },
          ],
        });
      }),
    );
    renderInsightsPanel();

    // last_run_ago_secs = null → 从未执行
    await waitFor(() => {
      expect(screen.getByText('从未执行')).toBeInTheDocument();
    });

    // last_run_ago_secs = 183 → X 分 Y 秒前
    expect(screen.getByText('3 分 3 秒前')).toBeInTheDocument();

    // last_run_ago_secs = 3600 → X 分前
    expect(screen.getByText('60 分前')).toBeInTheDocument();
  });

  it('shows error state with retry button when both scheduler and usage fail', async () => {
    // 一处失败不再拖垮整面板（allSettled 分区容错）：仅双失败整体报错
    server.use(
      http.get('/api/v1/scheduler/status', () => {
        return new HttpResponse(null, { status: 500 });
      }),
      http.get('/api/v1/stats', () => {
        return new HttpResponse(null, { status: 500 });
      }),
    );

    renderInsightsPanel();

    await waitFor(() => {
      expect(screen.getByRole('alert')).toBeInTheDocument();
    });
    expect(screen.getByRole('alert').textContent).toContain('HTTP 500');
    expect(screen.getByRole('button', { name: '重试' })).toBeInTheDocument();
    // 错误时不渲染任务表与统计卡
    expect(screen.queryByText('摘要生成')).not.toBeInTheDocument();
    expect(screen.queryByText('124')).not.toBeInTheDocument();
  });

  it('shows empty state when scheduler has no tasks', async () => {
    server.use(
      http.get('/api/v1/scheduler/status', () => {
        return HttpResponse.json({ ...mockSchedulerStatus, tasks: [] });
      }),
    );

    renderInsightsPanel();

    await waitFor(() => {
      expect(
        screen.getByText('调度器未装配（无启用的模型服务时不运行定时任务）'),
      ).toBeInTheDocument();
    });
    expect(screen.queryByText('摘要生成')).not.toBeInTheDocument();
    // 统计卡仍然渲染
    expect(screen.getByText('124')).toBeInTheDocument();
  });

  it('shows 未运行 badge when scheduler is not running', async () => {
    server.use(
      http.get('/api/v1/scheduler/status', () => {
        return HttpResponse.json({ ...mockSchedulerStatus, running: false });
      }),
    );

    renderInsightsPanel();

    await waitFor(() => {
      expect(screen.getByText('未运行')).toBeInTheDocument();
    });
    expect(screen.getByText('摘要生成')).toBeInTheDocument();
    expect(screen.getByText('124')).toBeInTheDocument();
  });

  it('refreshes data when clicking the refresh button', async () => {
    const user = userEvent.setup();
    let callCount = 0;
    server.use(
      http.get('/api/v1/stats', () => {
        callCount += 1;
        return HttpResponse.json(
          callCount === 1 ? mockUsageStats : { ...mockUsageStats, total_searches: 999 },
        );
      }),
    );

    renderInsightsPanel();

    await waitFor(() => {
      expect(screen.getByText('230')).toBeInTheDocument();
    });

    // 手动刷新 → 新数据出现
    await user.click(screen.getByRole('button', { name: /刷新/ }));

    await waitFor(() => {
      expect(screen.getByText('999')).toBeInTheDocument();
    });
    expect(callCount).toBe(2);
  });
});
