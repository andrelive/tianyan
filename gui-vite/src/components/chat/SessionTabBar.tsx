import { useMemo } from 'react';
import { HelpCircle } from 'lucide-react';
import { useAppStore } from '@/lib/store';
import { useOpenSession } from '@/hooks/use-open-session';
import { cn } from '@/lib/utils';
import type { Session } from '@/lib/types';

/**
 * 最近会话切换条（主区标题行内）：MRU 最近打开的会话（上限 6），点击即切。
 *
 * 与侧栏共用同一份状态标（运行中 / 等待回答 / 后台任务数）；已删除的会话
 * 自动过滤（持久化恢复后残留的 id 不渲染）。空列表不占位。
 */
export default function SessionTabBar() {
  const currentSessionId = useAppStore((s) => s.currentSessionId);
  const recentIds = useAppStore((s) => s.recentSessionIds);
  const sessions = useAppStore((s) => s.sessions);
  const turnState = useAppStore((s) => s.turnState);
  const tasks = useAppStore((s) => s.backgroundTasks);
  const openSession = useOpenSession();

  const items = useMemo(() => {
    const byId = new Map(sessions.map((s) => [s.id, s]));
    const taskCounts = new Map<string, number>();
    for (const t of tasks) {
      if (t.status !== 'pending' && t.status !== 'running') continue;
      const sid = t.parent_session_id;
      if (!sid) continue;
      taskCounts.set(sid, (taskCounts.get(sid) ?? 0) + 1);
    }
    return recentIds
      .map((id) => byId.get(id))
      .filter((s): s is Session => Boolean(s))
      .map((session) => {
        const ts = turnState[session.id];
        const running = ts?.state === 'running';
        return {
          session,
          running,
          pendingAsk: running && ts?.pendingAsk === true,
          taskCount: taskCounts.get(session.id) ?? 0,
        };
      });
  }, [recentIds, sessions, turnState, tasks]);

  if (items.length === 0) return null;

  return (
    <div
      role="tablist"
      aria-label="最近会话"
      className="flex items-center gap-1 min-w-0 flex-1 overflow-x-auto"
    >
      {items.map(({ session, running, pendingAsk, taskCount }) => {
        const active = session.id === currentSessionId;
        return (
          <button
            key={session.id}
            type="button"
            role="tab"
            aria-selected={active}
            title={session.title || '新对话'}
            onClick={() => openSession(session.id)}
            className={cn(
              'flex items-center gap-1.5 px-2.5 py-1 rounded-full border text-xs whitespace-nowrap shrink-0 max-w-[160px] transition-colors',
              active
                ? 'border-blue-500 bg-blue-50 text-blue-700 dark:bg-blue-900/20 dark:text-blue-300'
                : 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]',
            )}
          >
            {pendingAsk && (
              <HelpCircle
                size={11}
                className="shrink-0 text-amber-600 dark:text-amber-400"
                aria-label="等待你的回答"
              />
            )}
            {running && !pendingAsk && (
              <span
                className="w-1.5 h-1.5 rounded-full bg-emerald-500 animate-pulse shrink-0"
                aria-label="运行中"
              />
            )}
            <span className="truncate">{session.title || '新对话'}</span>
            {taskCount > 0 && (
              <span className="shrink-0 min-w-[14px] h-[14px] px-0.5 inline-flex items-center justify-center rounded-full bg-red-500 text-white text-[9px] font-bold leading-none">
                {taskCount > 99 ? '99+' : taskCount}
              </span>
            )}
          </button>
        );
      })}
    </div>
  );
}
