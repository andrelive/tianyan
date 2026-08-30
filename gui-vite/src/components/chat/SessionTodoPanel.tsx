import { useState, useCallback } from 'react';
import { usePolling } from '@/hooks/use-polling';
import { fetchTodos, fetchGoals } from '@/lib/api-client';
import type { GoalWithProgress, TodoItem } from '@/lib/types';
import {
  ChevronDown,
  ChevronUp,
  Circle,
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
 * 数据按会话绑定）：只展示当前会话的活跃条目——全部完成/删除后面板整体
 * 消失（临时语义，非全局计划页）。
 */
export default function SessionTodoPanel({ sessionId }: { sessionId: string | null }) {
  const [todos, setTodos] = useState<TodoItem[]>([]);
  const [goals, setGoals] = useState<GoalWithProgress[]>([]);
  const [expanded, setExpanded] = useState(true);

  const poll = useCallback(async () => {
    try {
      const [t, g] = await Promise.all([fetchTodos(sessionId!), fetchGoals(sessionId!)]);
      // 临时语义：只保留活跃条目（completed 即从面板消失）
      setTodos(t.todos.filter((x) => x.status !== 'completed'));
      setGoals(g.goals.filter((x) => x.goal.status === 'active'));
    } catch {
      /* 轮询失败静默保留旧数据 */
    }
  }, [sessionId]);
  usePolling(poll, POLL_INTERVAL_MS, { enabled: !!sessionId });

  if (!sessionId || (todos.length === 0 && goals.length === 0)) return null;

  const inProgress = todos.filter((t) => t.status === 'in_progress').length;
  const doneCount = todos.length - inProgress;

  return (
    <div className="shrink-0 border-t border-[var(--color-border)] bg-[var(--color-bg-secondary)] px-4 py-2">
      <div className="max-w-3xl mx-auto">
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
            {todos.length} 项
            {inProgress > 0 && `（进行中 ${inProgress}）`}
            {todos.length > 0 && inProgress < todos.length && ` · 完成 ${doneCount}`}
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
            {/* 活跃待办 */}
            {todos.map((t) => (
              <div
                key={t.id}
                className="flex items-center gap-2 text-xs text-[var(--color-text-secondary)]"
              >
                {t.status === 'in_progress' ? (
                  <Loader2 size={12} className="shrink-0 animate-spin text-blue-500" />
                ) : t.status === 'pending' ? (
                  <CircleDotDashed size={12} className="shrink-0 text-[var(--color-text-tertiary)]" />
                ) : (
                  <Circle size={12} className="shrink-0 text-[var(--color-text-tertiary)]" />
                )}
                <span
                  className={
                    'min-w-0 flex-1 truncate' +
                    (t.status === 'in_progress'
                      ? ' text-[var(--color-text-primary)] font-medium'
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
