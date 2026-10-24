import { useState, useCallback, useRef } from 'react';
import { usePolling } from '@/hooks/use-polling';
import { fetchTodos, fetchGoals } from '@/lib/api-client';
import { readTodoSnapshot, writeTodoSnapshot, type TodoSnapshot } from '@/lib/session-todo-cache';
import { useAppStore } from '@/lib/store';
import {
  ChevronDown,
  ChevronUp,
  Circle,
  CircleCheck,
  CircleDotDashed,
  ListTodo,
  Loader2,
  Target,
} from 'lucide-react';

/** 轮询间隔（智能体推进待办/目标后面板随之更新）。 */
const POLL_INTERVAL_MS = 3000;

/**
 * 会话待办/目标停靠面板（DSH todo 面板式，dock 在输入框上方、可折叠）。
 *
 * todo/goal 是智能体的会话内推理辅助工具（`todo`/`goal` 动态工具写入，
 * 数据按会话绑定）：展示当前会话的全部待办——完成条目保留展示（划线
 * 样式，便于回看做了什么），子待办缩进挂靠父项；无任何条目后面板整体
 * 消失（临时语义，非全局计划页）。
 *
 * 宽度与输入框对齐（同 max-w-4xl 居中容器）。
 */
export default function SessionTodoPanel({ sessionId }: { sessionId: string | null }) {
  const [expanded, setExpanded] = useState(true);

  /**
   * 视图 = 「这是哪个会话的数据 + 数据本身」。
   *
   * `sessionId` 变化时**在渲染期同步换**（React 官方「props 变化时调整 state」
   * 模式）——先读缓存，命中即同帧换掉，未命中先清空。换成 effect 里换会先
   * paint 一帧**上个会话**的待办，正是要修的症状。
   */
  const [view, setView] = useState<{ sid: string | null; data: TodoSnapshot }>(() => ({
    sid: sessionId,
    data: readTodoSnapshot(sessionId),
  }));
  if (view.sid !== sessionId) {
    setView({ sid: sessionId, data: readTodoSnapshot(sessionId) });
  }

  // 轮询函数保持稳定身份（靠 ref 读最新目标），请求返回后用它做**竞态守卫**：
  // 请求在飞时会话已切走 → 只更新缓存，不覆盖当前会话的视图。
  const sessionRef = useRef(sessionId);
  sessionRef.current = sessionId;

  // D：「工具执行改动了待办/目标」的变脏计数（chat-slice 在 todo/goal 的
  // 工具结果落库时递增）——作为 resetKey 触发立即重取，不必等下一个 tick。
  const todosRevision = useAppStore((s) => s.todosRevision);

  const poll = useCallback(async () => {
    const sid = sessionRef.current;
    if (!sid) return;
    try {
      const [t, g] = await Promise.all([fetchTodos(sid), fetchGoals(sid)]);
      // 完成条目保留展示（划线）；目标只展示活跃条目
      const snapshot: TodoSnapshot = {
        todos: t.todos,
        goals: g.goals.filter((x) => x.goal.status === 'active'),
      };
      writeTodoSnapshot(sid, snapshot);
      if (sessionRef.current === sid) setView({ sid, data: snapshot });
    } catch {
      /* 轮询失败静默保留旧数据 */
    }
  }, []);
  usePolling(poll, POLL_INTERVAL_MS, {
    enabled: !!sessionId,
    resetKeys: [sessionId, todosRevision],
  });

  const { todos, goals } = view.data;

  if (!sessionId || (todos.length === 0 && goals.length === 0)) return null;

  const inProgress = todos.filter((t) => t.status === 'in_progress').length;
  const doneCount = todos.filter((t) => t.status === 'completed').length;

  return (
    <div className="shrink-0 px-4 py-2">
      <div className="max-w-4xl mx-auto rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-secondary)] px-4 py-2">
        {/* 头部：标题 + 计数 + 折叠开关 */}
        <button
          type="button"
          onClick={() => setExpanded((v) => !v)}
          aria-expanded={expanded}
          aria-label="待办面板折叠开关"
          className="w-full flex items-center gap-1.5 text-xs font-medium text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]"
        >
          <ListTodo size={12} />
          <span>待办</span>
          <span className="text-[var(--color-text-tertiary)]">
            {todos.length} 项{inProgress > 0 && `（进行中 ${inProgress}）`}
            {doneCount > 0 && ` · 完成 ${doneCount}`}
          </span>
          <span className="flex-1" />
          {expanded ? <ChevronUp size={12} /> : <ChevronDown size={12} />}
        </button>

        {expanded && (
          <div className="mt-1.5 space-y-1">
            {/* 活跃目标条 */}
            {goals.map(({ goal, progress, todo_total, todo_done }) => (
              <div
                key={goal.id}
                className="flex items-center gap-2 text-xs text-[var(--color-text-secondary)]"
              >
                <Target size={12} className="shrink-0 text-blue-500" />
                <span className="min-w-0 flex-1 truncate" title={goal.title}>
                  {goal.title}
                </span>
                <span className="text-[var(--color-text-tertiary)] shrink-0">
                  {todo_total > 0 ? `${progress}%（${todo_done}/${todo_total}）` : '进行中'}
                </span>
              </div>
            ))}
            {/* 全部待办（完成项划线保留；子待办缩进） */}
            {todos.map((t) => (
              <div
                key={t.id}
                className={
                  'flex items-center gap-2 text-xs' +
                  (t.parent_id ? ' pl-5' : '') +
                  (t.status === 'completed'
                    ? ' text-[var(--color-text-tertiary)] line-through decoration-[var(--color-text-tertiary)]'
                    : ' text-[var(--color-text-secondary)]')
                }
              >
                {t.status === 'completed' ? (
                  <CircleCheck
                    size={12}
                    className="shrink-0 no-underline text-[var(--color-success)]"
                  />
                ) : t.status === 'in_progress' ? (
                  <Loader2 size={12} className="shrink-0 animate-spin text-blue-500" />
                ) : t.status === 'pending' ? (
                  <CircleDotDashed
                    size={12}
                    className="shrink-0 text-[var(--color-text-tertiary)]"
                  />
                ) : (
                  <Circle size={12} className="shrink-0 text-[var(--color-text-tertiary)]" />
                )}
                <span
                  className={
                    'min-w-0 flex-1 truncate' +
                    (t.status === 'in_progress'
                      ? ' text-[var(--color-text-primary)] font-medium no-underline'
                      : '')
                  }
                  title={t.title}
                >
                  {t.title}
                </span>
                {t.status === 'pending' && (
                  <span className="text-[var(--color-text-tertiary)] shrink-0">待开始</span>
                )}
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
