import { describe, it, expect } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import SessionTodoPanel from '../SessionTodoPanel';
import { server } from '@/test/mocks/server';
import { mockTodos } from '@/test/mocks/handlers';
import type { TodoItem } from '@/lib/types';

/** 会话待办/目标停靠面板（会话绑定 + 只展示活跃条目 + 空即整面板消失）。 */

describe('SessionTodoPanel', () => {
  it('renders nothing without a session (new chat)', () => {
    const { container } = render(<SessionTodoPanel sessionId={null} />);
    expect(container).toBeEmptyDOMElement();
  });

  it('shows active session todos and goals, hiding other sessions and completed', async () => {
    render(<SessionTodoPanel sessionId="session-1" />);

    await waitFor(() => {
      expect(screen.getByText('进行中的待办')).toBeInTheDocument();
    });
    expect(screen.getByText('待开始的待办')).toBeInTheDocument();
    expect(screen.getByText('会话内目标')).toBeInTheDocument();

    // 头部计数：2 项活跃 + 进行中 1（计数与「待办」标题分属两个 span）
    expect(screen.getByText(/2 项（进行中 1）/)).toBeInTheDocument();

    // 会话绑定：其他会话的待办不出现
    expect(screen.queryByText('其他会话的待办')).not.toBeInTheDocument();
  });

  it('disappears entirely when all session todos are completed', async () => {
    // 覆盖：session-1 全部待办已完成
    const allCompleted: TodoItem[] = mockTodos.map((t) =>
      t.session_id === 'session-1' ? { ...t, status: 'completed' as const } : t,
    );
    server.use(
      http.get('*/api/v1/todos', () => {
        return HttpResponse.json({ todos: allCompleted, total: allCompleted.length });
      }),
    );

    const { container } = render(<SessionTodoPanel sessionId="session-1" />);
    // 目标也需隐藏：goals mock 仍为 active —— 覆盖为空
    server.use(
      http.get('*/api/v1/goals', () => {
        return HttpResponse.json({ goals: [], total: 0 });
      }),
    );
    await waitFor(() => {
      expect(container).toBeEmptyDOMElement();
    });
  });
});
