import { useState, useCallback } from 'react';
import { Link } from 'react-router-dom';
import ScheduledTasksSection from './ScheduledTasksSection';
import { usePolling } from '@/hooks/use-polling';
import { Spinner } from '@/components/ui/Spinner';
import { EmptyState } from '@/components/ui/EmptyState';
import { fetchTasks, cancelTask, fetchSchedulerStatus } from '@/lib/api-client';
import { formatTimestamp } from '@/lib/utils';
import type { BackgroundTask, SchedulerStatus, TaskStatus } from '@/lib/types';
import {
  Loader2,
  ListTodo,
  RefreshCw,
  XCircle,
  CircleDashed,
  CircleCheck,
  CircleX,
  Cpu,
  CalendarClock,
  Layers,
} from 'lucide-react';

const POLL_INTERVAL_MS = 3000;
const RESULT_MAX_LEN = 120;

/** 任务状态 → 中文标签。 */
const STATUS_LABELS: Record<TaskStatus, string> = {
  pending: '等待中',
  running: '运行中',
  completed: '已完成',
  failed: '失败',
  cancelled: '已取消',
};

/** 任务状态 → badge 颜色（Tailwind 明暗双主题）。 */
const STATUS_BADGE_CLASSES: Record<TaskStatus, string> = {
  pending:
    'bg-gray-50 dark:bg-gray-900/30 text-gray-600 dark:text-gray-300 border-gray-200 dark:border-gray-700',
  running:
    'bg-blue-50 dark:bg-blue-900/30 text-blue-700 dark:text-blue-300 border-blue-200 dark:border-blue-800',
  completed:
    'bg-green-50 dark:bg-green-900/30 text-green-700 dark:text-green-300 border-green-200 dark:border-green-800',
  failed:
    'bg-red-50 dark:bg-red-900/30 text-red-700 dark:text-red-300 border-red-200 dark:border-red-800',
  cancelled:
    'bg-gray-50 dark:bg-gray-900/30 text-gray-500 dark:text-gray-400 border-gray-200 dark:border-gray-700 italic',
};

/** 任务状态徽标（running 带脉冲动画）。 */
function StatusBadge({ status }: { status: TaskStatus }) {
  const pulse = status === 'running';
  return (
    <span
      className={`inline-flex items-center gap-1 px-1.5 py-0.5 text-xs rounded border ${STATUS_BADGE_CLASSES[status]}`}
    >
      {pulse && <span className="w-1.5 h-1.5 rounded-full bg-blue-500 animate-pulse" />}
      {STATUS_LABELS[status] ?? status}
    </span>
  );
}

/** 截断长文本为摘要（超过长度补 …）。 */
function truncate(text: string, maxLen = RESULT_MAX_LEN): string {
  if (text.length <= maxLen) return text;
  return `${text.slice(0, maxLen)}…`;
}

/** 可展开文本（超长结果/错误：默认截断，可展开全文——P1：长产出不再不可见）。 */
function ExpandableText({ text, maxLen = RESULT_MAX_LEN }: { text: string; maxLen?: number }) {
  const [open, setOpen] = useState(false);
  const long = text.length > maxLen;
  return (
    <span>
      {long && !open ? truncate(text, maxLen) : text}
      {long && (
        <button
          type="button"
          onClick={() => setOpen((v) => !v)}
          className="ml-1.5 text-blue-600 dark:text-blue-400 underline hover:opacity-80"
        >
          {open ? '收起' : '展开'}
        </button>
      )}
    </span>
  );
}

/** 距上次执行秒数 → 可读文本。 */
function agoText(secs: number | null): string {
  if (secs === null) return '从未执行';
  if (secs < 60) return `${secs} 秒前`;
  if (secs < 3600) return `${Math.floor(secs / 60)} 分钟前`;
  if (secs < 86400) return `${Math.floor(secs / 3600)} 小时前`;
  return `${Math.floor(secs / 86400)} 天前`;
}

/** 内置任务（调度器 cron 任务）列表：只读状态展示。 */
function BuiltinTasksSection({ status }: { status: SchedulerStatus | null }) {
  if (!status) {
    return <EmptyState icon={Cpu} title="调度器未运行" hint="未配置启用的模型服务时内置任务不启动" />;
  }
  if (status.tasks.length === 0) {
    return <EmptyState icon={Cpu} title="暂无内置任务" />;
  }
  return (
    <div className="space-y-2">
      {status.tasks.map((t) => (
        <div
          key={t.id}
          className="p-4 rounded-lg bg-[var(--color-bg-secondary)] border border-[var(--color-border)]"
        >
          <div className="flex items-start justify-between gap-3">
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <span className="text-xs text-[var(--color-text-tertiary)] font-mono shrink-0">
                  {t.id}
                </span>
                <p className="text-sm text-[var(--color-text-primary)]">{t.name}</p>
              </div>
              <div className="flex items-center gap-2 mt-1.5 text-xs text-[var(--color-text-tertiary)]">
                <span className="px-1.5 py-0.5 text-[10px] rounded bg-blue-50 dark:bg-blue-900/30 text-blue-700 dark:text-blue-300 border border-blue-200 dark:border-blue-800 font-mono">
                  {t.cron_expression}
                </span>
                <span>已执行 {t.run_count} 次</span>
                <span>上次 {agoText(t.last_run_ago_secs)}</span>
              </div>
            </div>
          </div>
        </div>
      ))}
    </div>
  );
}

/** 面板 tab 定义。 */
type PanelTab = 'builtin' | 'scheduled' | 'background';

const TABS: { id: PanelTab; label: string; icon: typeof Cpu }[] = [
  { id: 'builtin', label: '内置任务', icon: Cpu },
  { id: 'scheduled', label: '定时任务', icon: CalendarClock },
  { id: 'background', label: '后台任务', icon: Layers },
];

export default function TasksPanel() {
  const [tab, setTab] = useState<PanelTab>('builtin');
  const [tasks, setTasks] = useState<BackgroundTask[]>([]);
  const [scheduler, setScheduler] = useState<SchedulerStatus | null>(null);
  // 正在取消的 task_id，用于禁用按钮防重复点击
  const [cancellingId, setCancellingId] = useState<string | null>(null);

  // 挂载后立即加载 + 每 3s 轮询（后台任务状态变化）；轮询失败静默保留旧数据
  const pollTasks = useCallback(async () => {
    try {
      const data = await fetchTasks();
      setTasks(data);
    } catch {
      // 轮询失败静默保留旧数据，面板不因后端异常崩溃
    }
    try {
      const data = await fetchSchedulerStatus();
      setScheduler(data);
    } catch {
      /* 静默 */
    }
  }, []);
  const { loading, refresh } = usePolling(pollTasks, POLL_INTERVAL_MS);

  const handleCancel = useCallback(
    async (taskId: string) => {
      setCancellingId(taskId);
      try {
        await cancelTask(taskId);
        // 取消后立即刷新列表
        await refresh();
      } catch {
        // 取消失败（如任务已结束）：保留旧数据，下次轮询自然更新
      } finally {
        setCancellingId(null);
      }
    },
    [refresh],
  );

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="flex items-center justify-between px-6 py-4 border-b border-[var(--color-border)]">
        <h2 className="text-lg font-semibold text-[var(--color-text-primary)]">任务</h2>
        <button
          onClick={() => void refresh(true)}
          disabled={loading}
          className="flex items-center gap-1 px-2 py-1 text-xs rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50"
        >
          <RefreshCw size={12} />
          刷新
        </button>
      </div>

      {/* Tab bar：内置任务（调度器 cron）/ 定时任务（用户创建）/ 后台任务（委托） */}
      <div
        role="tablist"
        aria-label="任务分类"
        className="flex items-center gap-1 px-4 pt-3 border-b border-[var(--color-border)]"
      >
        {TABS.map((t) => {
          const Icon = t.icon;
          const active = tab === t.id;
          return (
            <button
              key={t.id}
              role="tab"
              aria-selected={active}
              onClick={() => setTab(t.id)}
              className={`flex items-center gap-1.5 px-3 py-1.5 text-xs rounded-t-md border-b-2 transition-colors ${
                active
                  ? 'border-accent text-accent font-medium'
                  : 'border-transparent text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]'
              }`}
            >
              <Icon size={13} />
              {t.label}
            </button>
          );
        })}
      </div>

      {/* Body */}
      <div className="flex-1 overflow-y-auto p-6">
        {tab === 'builtin' && (
          <div className="space-y-2">
            <p className="text-xs text-[var(--color-text-tertiary)]">
              系统内置的周期任务（摘要生成 / 自演化 / 垃圾回收 / 统计刷盘等），由调度器按 cron 自动执行。
            </p>
            {loading && !scheduler ? <Spinner /> : <BuiltinTasksSection status={scheduler} />}
          </div>
        )}

        {tab === 'scheduled' && <ScheduledTasksSection />}

        {tab === 'background' && (
          <div className="space-y-2">
            <p className="text-xs text-[var(--color-text-tertiary)]">
              委托给子智能体的后台任务（delegate_to_agent background），断开连接后继续运行，可随时取消。
            </p>
            {loading && tasks.length === 0 && <Spinner />}

            {!loading && tasks.length === 0 && <EmptyState icon={ListTodo} title="暂无后台任务" />}

            {tasks.length > 0 && (
              <div className="space-y-2">
                {tasks.map((task) => {
                  const cancellable = task.status === 'running' || task.status === 'pending';
                  const cancelling = cancellingId === task.id;
                  return (
                    <div
                      key={task.id}
                      className="p-4 rounded-lg bg-[var(--color-bg-secondary)] border border-[var(--color-border)]"
                    >
                      <div className="flex items-start justify-between gap-3">
                        <div className="min-w-0 flex-1">
                          <div className="flex items-center gap-2">
                            <span className="text-xs text-[var(--color-text-tertiary)] font-mono shrink-0">
                              #{task.seq}
                            </span>
                            <p className="text-sm text-[var(--color-text-primary)] break-all">
                              {task.description || task.id}
                            </p>
                          </div>
                          <div className="flex items-center gap-2 mt-1.5 text-xs text-[var(--color-text-tertiary)]">
                            <StatusBadge status={task.status} />
                            {task.parent_session_id ? (
                              <Link
                                to={'/chat/' + task.parent_session_id}
                                className="hover:text-blue-600 underline underline-offset-2"
                                title="跳转到该会话"
                              >
                                会话{' '}
                                {task.parent_session_id.length > 16
                                  ? task.parent_session_id.slice(0, 12) + '…'
                                  : task.parent_session_id}
                              </Link>
                            ) : (
                              <span>会话 —</span>
                            )}
                            <span>{formatTimestamp(task.created_at)}</span>
                            {task.completed_at !== null && (
                              <span>完成于 {formatTimestamp(task.completed_at)}</span>
                            )}
                          </div>
                        </div>
                        {cancellable && (
                          <button
                            onClick={() => void handleCancel(task.id)}
                            disabled={cancelling}
                            aria-label={`取消任务 ${task.description || task.id}`}
                            className="flex items-center gap-1 px-2.5 py-1 text-xs rounded border border-red-200 dark:border-red-800 text-red-600 dark:text-red-400 hover:bg-red-50 dark:hover:bg-red-900/30 disabled:opacity-50 shrink-0"
                          >
                            {cancelling ? (
                              <Loader2 size={12} className="animate-spin" />
                            ) : (
                              <XCircle size={12} />
                            )}
                            取消
                          </button>
                        )}
                      </div>

                      {/* 终态任务：结果摘要或错误 */}
                      {task.status === 'completed' && task.result !== null && (
                        <div className="flex items-start gap-2 mt-2 pt-2 border-t border-[var(--color-border)]">
                          <CircleCheck size={14} className="mt-0.5 shrink-0 text-green-500" />
                          <p className="text-xs text-[var(--color-text-secondary)] break-all">
                            <ExpandableText text={task.result} />
                          </p>
                        </div>
                      )}
                      {task.status === 'failed' && task.error !== null && (
                        <div className="flex items-start gap-2 mt-2 pt-2 border-t border-[var(--color-border)]">
                          <CircleX size={14} className="mt-0.5 shrink-0 text-red-500" />
                          <p className="text-xs text-red-600 dark:text-red-400 break-all">
                            <ExpandableText text={task.error} />
                          </p>
                        </div>
                      )}
                      {task.status === 'cancelled' && (
                        <div className="flex items-start gap-2 mt-2 pt-2 border-t border-[var(--color-border)]">
                          <CircleDashed size={14} className="mt-0.5 shrink-0 text-gray-400" />
                          <p className="text-xs text-[var(--color-text-tertiary)] italic">任务已取消</p>
                        </div>
                      )}
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
