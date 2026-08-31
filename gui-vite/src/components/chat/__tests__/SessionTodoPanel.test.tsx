import { describe, it, expect } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import SessionTodoPanel from '../SessionTodoPanel';
import { server } from '@/test/mocks/server';
import { mockTodos } from '@/test/mocks/handlers';

/** 会话待办/目标停靠面板（会话绑定 + 完成条目划线保留 + 空即整面板消失）。 */

describe('SessionTodoPanel', () => {
  it('renders nothing without a session (new chat)', () => {
    const { container } = render(<SessionTodoPanel sessionId={null} />);
    expect(container).toBeEmptyDOMElement();
  });

  it('shows session todos incl. completed styling, hiding other sessions', async () => {
    render(<SessionTodoPanel sessionId="session-1" />);

    await waitFor(() => {
      expect(screen.getByText('进行中的待办')).toBeInTheDocument();
    });
    expect(screen.getByText('待开始的待办')).toBeInTheDocument();
    expect(screen.getByText('会话内目标')).toBeInTheDocument();

    // 头部计数：3 项（进行中 1） · 完成 1
    expect(screen.getByText(/3 项（进行中 1）/)).toBeInTheDocument();

    // 完成条目保留展示（划线样式），不消失
    const doneRow = screen.getByText('已完成的待办').closest('div');
    expect(doneRow?.className).toContain('line-through');

    // 会话绑定：其他会话的待办不出现
    expect(screen.queryByText('其他会话的待办')).not.toBeInTheDocument();
  });

  it('disappears entirely when the session has no todos and no goals', async () => {
    // 覆盖：session-1 无任何待办、无活跃目标 → 面板整体消失
    server.use(
      http.get('*/api/v1/todos', () => {
        return HttpResponse.json({ todos: [], total: 0 });
      }),
    );
    server.use(
      http.get('*/api/v1/goals', () => {
        return HttpResponse.json({ goals: [], total: 0 });
      }),
    );

    const { container } = render(<SessionTodoPanel sessionId="session-1" />);
    await waitFor(() => {
      expect(container).toBeEmptyDOMElement();
    });
  });

  it('keeps the panel visible with only completed todos (shown as done)', async () => {
    const allCompleted = mockTodos
      .filter((t) => t.session_id === 'session-1')
      .map((t) => ({ ...t, status: 'completed' as const }));
    server.use(
      http.get('*/api/v1/todos', () => {
        return HttpResponse.json({ todos: allCompleted, total: allCompleted.length });
      }),
    );
    server.use(
      http.get('*/api/v1/goals', () => {
        return HttpResponse.json({ goals: [], total: 0 });
      }),
    );

    render(<SessionTodoPanel sessionId="session-1" />);
    await waitFor(() => {
      expect(screen.getByText('进行中的待办')).toBeInTheDocument();
    });
    // 全部完成后面板仍可见（划线展示），头部显示完成数
    expect(screen.getByText(/完成 3/)).toBeInTheDocument();
  });
});
