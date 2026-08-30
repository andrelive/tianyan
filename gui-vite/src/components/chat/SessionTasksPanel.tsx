import { useState, useCallback } from 'react';
import { usePolling } from '@/hooks/use-polling';
import { fetchTasks, cancelTask } from '@/lib/api-client';
import type { BackgroundTask } from '@/lib/types';
import { Loader2, Layers, XCircle } from 'lucide-react';

/** 轮询间隔（后台任务状态变化）。 */
const POLL_INTERVAL_MS = 3000;

/**
 * 会话后台任务停靠条（DSH 式按会话归属）：展示当前会话发起、仍在运行的
 * 后台任务（delegate_to_agent background / 后台终端），可随时取消。
 *
 * 临时语义：只展示 pending/running——任务结束即从条内消失，结果由主
 * agent 自动汇总进会话流（ADR-013 唤醒轮）；无运行中任务时整条不渲染。
 */
export default function SessionTasksPanel({ sessionId }: { sessionId: string | null }) {
  const [tasks, setTasks] = useState<BackgroundTask[]>([]);
  const [cancellingId, setCancellingId] = useState<string | null>(null);

  const poll = useCallback(async () => {
    try {
      const data = await fetchTasks();
      setTasks(data);
    } catch {
      /* 轮询失败静默保留旧数据 */
    }
  }, []);
  usePolling(poll, POLL_INTERVAL_MS, { enabled: !!sessionId });

  const handleCancel = useCallback(async (taskId: string) => {
    setCancellingId(taskId);
    try {
      await cancelTask(taskId);
      await poll();
    } catch {
      /* 取消失败（如任务已结束）：下次轮询自然更新 */
    } finally {
      setCancellingId(null);
    }
  }, [poll]);

  const mine = tasks.filter(
    (t) => t.parent_session_id === sessionId && (t.status === 'pending' || t.status === 'running'),
  );
  if (!sessionId || mine.length === 0) return null;

  return (
    <div className="shrink-0 border-t border-[var(--color-border)] bg-[var(--color-bg-secondary)] px-4 py-2">
      <div className="max-w-3xl mx-auto space-y-1.5">
        <p className="flex items-center gap-1.5 text-xs font-medium text-[var(--color-text-secondary)]">
          <Layers size={12} />
          后台任务 {mine.length} 个运行中
        </p>
        {mine.map((t) => (
          <div
            key={t.id}
            className="flex items-center gap-2 text-xs text-[var(--color-text-secondary)]"
          >
            <span
              className="w-1.5 h-1.5 rounded-full bg-blue-500 animate-pulse shrink-0"
              aria-hidden
            />
            <span className="font-mono text-[var(--color-text-tertiary)] shrink-0">#{t.seq}</span>
            <span className="min-w-0 flex-1 truncate" title={t.description || t.id}>
              {t.description || t.id}
            </span>
            {t.status === 'pending' && (
              <span className="text-[var(--color-text-tertiary)] shrink-0">等待中</span>
            )}
            <button
              type="button"
              onClick={() => void handleCancel(t.id)}
              disabled={cancellingId === t.id}
              aria-label={'取消后台任务 ' + (t.description || t.id)}
              className="flex items-center gap-1 px-1.5 py-0.5 rounded border border-[var(--color-border)] text-[var(--color-text-tertiary)] hover:text-red-500 hover:border-red-200 dark:hover:border-red-800 disabled:opacity-50 shrink-0"
            >
              {cancellingId === t.id ? (
                <Loader2 size={11} className="animate-spin" />
              ) : (
                <XCircle size={11} />
              )}
              取消
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}
