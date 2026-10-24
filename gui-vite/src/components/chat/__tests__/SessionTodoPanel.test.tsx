import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor, act } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import SessionTodoPanel from '../SessionTodoPanel';
import { clearTodoSnapshotCache } from '@/lib/session-todo-cache';
import { server } from '@/test/mocks/server';
import { mockTodos } from '@/test/mocks/handlers';
import { useAppStore } from '@/lib/store';

/** 会话待办/目标停靠面板（会话绑定 + 完成条目划线保留 + 空即整面板消失）。 */

beforeEach(() => {
  // 模块级快照缓存跨用例共享（切换会话零延迟的载体）——清掉，避免用例间互相命中
  clearTodoSnapshotCache();
  useAppStore.setState(useAppStore.getInitialState());
});

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

  // ── 切换会话：旧数据不得滞留（此前要等下一个 3s 轮询 tick）──

  it('switches to the new session data immediately via resetKeys (no 3s tick)', async () => {
    const { rerender } = render(<SessionTodoPanel sessionId="session-1" />);
    await waitFor(() => {
      expect(screen.getByText('进行中的待办')).toBeInTheDocument();
    });

    // 切到 session-2：**不推进任何定时器**——只能靠 resetKeys 的立即重取生效。
    // 旧实现（effect 依赖里没有会话）在这里会一直显示 session-1 的待办直到超时。
    rerender(<SessionTodoPanel sessionId="session-2" />);
    await waitFor(() => {
      expect(screen.getByText('其他会话的待办')).toBeInTheDocument();
    });
    expect(screen.queryByText('进行中的待办')).not.toBeInTheDocument();
  });

  it('renders the cached snapshot synchronously when switching back (zero-delay)', async () => {
    const { rerender } = render(<SessionTodoPanel sessionId="session-1" />);
    await waitFor(() => {
      expect(screen.getByText('进行中的待办')).toBeInTheDocument();
    });

    rerender(<SessionTodoPanel sessionId="session-2" />);
    await waitFor(() => {
      expect(screen.getByText('其他会话的待办')).toBeInTheDocument();
    });

    // 切回 session-1：**同步**断言（没有任何 await）——缓存命中必须同一帧可见，
    // 不能等请求。无缓存的实现此时视图为空 → 面板 return null → 断言失败。
    rerender(<SessionTodoPanel sessionId="session-1" />);
    expect(screen.getByText('进行中的待办')).toBeInTheDocument();
  });

  it('ignores a stale in-flight response from the previous session (race guard)', async () => {
    server.use(
      http.get('*/api/v1/todos', async ({ request }) => {
        const sid = new URL(request.url).searchParams.get('session_id');
        // session-1 的响应故意慢：它会在切走之后才回来
        if (sid === 'session-1') await new Promise((r) => setTimeout(r, 150));
        const todos = mockTodos.filter((t) => t.session_id === sid);
        return HttpResponse.json({ todos, total: todos.length });
      }),
    );

    const { rerender } = render(<SessionTodoPanel sessionId="session-1" />);
    // session-1 的请求仍在飞（150ms 后返回）→ 立刻切走
    rerender(<SessionTodoPanel sessionId="session-2" />);
    await waitFor(() => {
      expect(screen.getByText('其他会话的待办')).toBeInTheDocument();
    });

    // 等 session-1 的迟到响应落地
    await act(async () => {
      await new Promise((r) => setTimeout(r, 300));
    });

    // 竞态守卫：迟到数据只写缓存，不得覆盖当前会话视图
    expect(screen.getByText('其他会话的待办')).toBeInTheDocument();
    expect(screen.queryByText('进行中的待办')).not.toBeInTheDocument();
  });

  // ── D：智能体用 todo/goal 工具推进待办 → 面板立即重取（此前最多滞后 3s）──

  it('refetches when todosRevision bumps (agent touched todo/goal)', async () => {
    render(<SessionTodoPanel sessionId="session-1" />);
    await waitFor(() => {
      expect(screen.getByText('进行中的待办')).toBeInTheDocument();
    });

    // 服务端数据变了（智能体刚推进待办）——不推进定时器，只 bump 变脏计数
    server.use(
      http.get('*/api/v1/todos', () =>
        HttpResponse.json({
          todos: [{ ...mockTodos[0], title: '智能体刚改的待办' }],
          total: 1,
        }),
      ),
    );
    act(() => {
      useAppStore.setState({ todosRevision: useAppStore.getState().todosRevision + 1 });
    });

    await waitFor(() => {
      expect(screen.getByText('智能体刚改的待办')).toBeInTheDocument();
    });
  });
});
