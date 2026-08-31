import { useState, useCallback } from 'react';
import { usePolling } from '@/hooks/use-polling';
import { fetchTasks, cancelTask } from '@/lib/api-client';
import type { BackgroundTask } from '@/lib/types';
import {
  CircleCheck,
  CircleX,
  Layers,
  Loader2,
  MinusCircle,
  XCircle,
} from 'lucide-react';

/** 轮询间隔（后台任务状态变化）。 */
const POLL_INTERVAL_MS = 3000;

/**
 * 会话后台任务停靠条（DSH 式按会话归属）：展示当前会话发起的后台任务
 * （delegate_to_agent background 委托 / execute_command background 终端命令）。
 *
 * 终态保留展示：运行中可取消；完成/失败/已取消以状态图标留在列表里
 * （回看做了什么、结果如何）——与待办面板的完成保留语义一致。任务结果
 * 的详细内容走会话流（ADR-013 完成通知 + 唤醒轮 agent 汇总）。
 * 无任何任务时整条不渲染。
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

  // 本会话任务，按创建时间排序（委托/命令两套注册表 seq 独立，统一按时间线）
  const mine = tasks
    .filter((t) => t.parent_session_id === sessionId)
    .sort((a, b) => a.created_at - b.created_at || a.seq - b.seq);
  if (!sessionId || mine.length === 0) return null;

  const runningCount = mine.filter(
    (t) => t.status === 'pending' || t.status === 'running',
  ).length;
  const doneCount = mine.length - runningCount;

  const statusLabel = (t: BackgroundTask) => {
    if (t.status === 'pending') return '等待中';
    if (t.status === 'completed') return '已完成';
    if (t.status === 'failed') return t.error ? '失败（' + t.error + '）' : '失败';
    if (t.status === 'cancelled') return '已取消';
    return null;
  };

  return (
    <div className="shrink-0 border-t border-[var(--color-border)] bg-[var(--color-bg-secondary)] px-4 py-2">
      <div className="max-w-4xl mx-auto space-y-1.5">
        <p className="flex items-center gap-1.5 text-xs font-medium text-[var(--color-text-secondary)]">
          <Layers size={12} />
          <span>后台任务</span>
          {runningCount > 0 && (
            <span className="text-[var(--color-text-tertiary)]">运行中 {runningCount}</span>
          )}
          {doneCount > 0 && (
            <span className="text-[var(--color-text-tertiary)]">已结束 {doneCount}</span>
          )}
        </p>
        {mine.map((t) => {
          const terminal =
            t.status === 'completed' || t.status === 'failed' || t.status === 'cancelled';
          const rowTone = terminal
            ? ' text-[var(--color-text-tertiary)]'
            : ' text-[var(--color-text-secondary)]';
          const label = statusLabel(t);
          return (
            <div
              key={t.id}
              className={'flex items-center gap-2 text-xs' + rowTone}
              title={t.error || t.description || t.id}
            >
              {t.status === 'running' ? (
                <Loader2 size={12} className="shrink-0 animate-spin text-blue-500" />
              ) : t.status === 'pending' ? (
                <Loader2 size={12} className="shrink-0 text-[var(--color-text-tertiary)]" />
              ) : t.status === 'completed' ? (
                <CircleCheck size={12} className="shrink-0 text-[var(--color-success)]" />
              ) : t.status === 'failed' ? (
                <CircleX size={12} className="shrink-0 text-[var(--color-error)]" />
              ) : (
                <MinusCircle size={12} className="shrink-0 text-[var(--color-text-tertiary)]" />
              )}
              <span
                className="px-1 rounded text-[10px] shrink-0 border border-[var(--color-border)] text-[var(--color-text-tertiary)]"
                title={t.kind === 'command' ? '后台终端命令' : '子代理委托'}
              >
                {t.kind === 'command' ? '终端' : '委托'}
              </span>
              <span className="min-w-0 flex-1 truncate font-mono" title={t.description || t.id}>
                {t.description || t.id}
              </span>
              {label && (
                <span
                  className={
                    'shrink-0' +
                    (t.status === 'failed'
                      ? ' text-[var(--color-error)]'
                      : ' text-[var(--color-text-tertiary)]')
                  }
                >
                  {label}
                </span>
              )}
              {!terminal && (
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
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
