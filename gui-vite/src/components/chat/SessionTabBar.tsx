import { useMemo } from 'react';
import { HelpCircle, X } from 'lucide-react';
import { useAppStore } from '@/lib/store';
import { useOpenSession } from '@/hooks/use-open-session';
import { cn } from '@/lib/utils';
import type { Session } from '@/lib/types';

/**
 * 打开的会话 tab 条（主区标题行内）：浏览器式交互——按打开先后排序
 *（append-only，切换不重排）、点击即切、× 关闭（关当前则激活邻位）。
 * 仅剩一个 tab 时隐藏关闭按钮（当前会话恒为 tab，不允许关空）。
 *
 * 与侧栏共用同一份状态标（运行中 / 等待回答 / 后台任务数）；已删除的会话
 * 自动过滤（持久化恢复后残留的 id 不渲染）。空列表不占位。
 */
export default function SessionTabBar() {
  const currentSessionId = useAppStore((s) => s.currentSessionId);
  const openIds = useAppStore((s) => s.openSessionIds);
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
    return openIds
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
  }, [openIds, sessions, turnState, tasks]);

  if (items.length === 0) return null;

  /** 关闭 tab：关当前 tab 时激活邻位（优先右侧同位置，其次左侧）——浏览器语义。 */
  const handleClose = (e: React.MouseEvent, id: string) => {
    e.stopPropagation();
    const st = useAppStore.getState();
    const ids = st.openSessionIds;
    const idx = ids.indexOf(id);
    const wasActive = st.currentSessionId === id;
    st.closeSessionTab(id);
    if (!wasActive || idx < 0) return;
    const remaining = ids.filter((x) => x !== id);
    const nextId = remaining[Math.min(idx, remaining.length - 1)];
    if (nextId) openSession(nextId);
  };

  return (
    <div
      role="tablist"
      aria-label="打开的会话"
      className="flex items-center gap-1 min-w-0 flex-1 overflow-x-auto"
    >
      {items.map(({ session, running, pendingAsk, taskCount }) => {
        const active = session.id === currentSessionId;
        return (
          <div
            key={session.id}
            role="tab"
            tabIndex={0}
            aria-selected={active}
            title={session.title || '新对话'}
            onClick={() => openSession(session.id)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault();
                openSession(session.id);
              }
            }}
            className={cn(
              'group flex items-center gap-1.5 py-1 rounded-full border text-xs whitespace-nowrap shrink-0 max-w-[180px] cursor-pointer transition-colors',
              items.length > 1 ? 'pl-2.5 pr-1' : 'px-2.5',
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
            {items.length > 1 && (
              <button
                type="button"
                onClick={(e) => handleClose(e, session.id)}
                title="关闭标签页"
                aria-label={`关闭 ${session.title || '新对话'}`}
                className={cn(
                  'shrink-0 -mr-0.5 p-0.5 rounded-full text-current transition-opacity',
                  'group-hover:opacity-100 focus-visible:opacity-100 hover:bg-black/10 dark:hover:bg-white/15',
                  active ? 'opacity-60' : 'opacity-0',
                )}
              >
                <X size={11} />
              </button>
            )}
          </div>
        );
      })}
    </div>
  );
}
