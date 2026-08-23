import { fetchSchedulerStatus, fetchUsageStats } from '@/lib/api-client';
import { formatNumber } from '@/lib/utils';
import { useResource } from '@/hooks/use-resource';
import { Spinner } from '@/components/ui/Spinner';
import { ErrorBanner } from '@/components/ui/ErrorBanner';
import type { SchedulerTaskStatus } from '@/lib/types';
import { RefreshCw, Gauge, Wrench, Zap, FileText, Search, Clock, Terminal } from 'lucide-react';

/** 距上次执行秒数 → 中文显示（null = 从未执行）。 */
function formatLastRun(secs: number | null): string {
  if (secs === null) return '从未执行';
  if (secs <= 60) return `${secs} 秒前`;
  const minutes = Math.floor(secs / 60);
  const seconds = secs % 60;
  if (seconds > 0) return `${minutes} 分 ${seconds} 秒前`;
  return `${minutes} 分前`;
}

/** 优先级 → badge 颜色（Tailwind 明暗双主题）。 */
const PRIORITY_BADGE_CLASSES: Record<string, string> = {
  Low: 'bg-gray-50 dark:bg-gray-800 text-gray-600 dark:text-gray-400 border-gray-200 dark:border-gray-700',
  Medium:
    'bg-blue-50 dark:bg-blue-900/30 text-blue-700 dark:text-blue-300 border-blue-200 dark:border-blue-800',
  High: 'bg-orange-50 dark:bg-orange-900/30 text-orange-700 dark:text-orange-300 border-orange-200 dark:border-orange-800',
};

function PriorityBadge({ priority }: { priority: string }) {
  const classes = PRIORITY_BADGE_CLASSES[priority] ?? PRIORITY_BADGE_CLASSES.Medium;
  return (
    <span className={`inline-flex items-center px-1.5 py-0.5 text-xs rounded border ${classes}`}>
      {priority}
    </span>
  );
}

/** 调度器运行状态徽标。 */
function RunningBadge({ running }: { running: boolean }) {
  const classes = running
    ? 'bg-green-50 dark:bg-green-900/30 text-green-700 dark:text-green-300 border-green-200 dark:border-green-800'
    : 'bg-gray-50 dark:bg-gray-800 text-gray-600 dark:text-gray-400 border-gray-200 dark:border-gray-700';
  return (
    <span
      className={`inline-flex items-center gap-1.5 px-2 py-0.5 text-xs rounded border ${classes}`}
    >
      <span
        className={`w-1.5 h-1.5 rounded-full ${running ? 'bg-green-500' : 'bg-gray-400'}`}
        aria-hidden="true"
      />
      {running ? '运行中' : '未运行'}
    </span>
  );
}

/** 单个定时任务行。 */
function TaskRow({ task }: { task: SchedulerTaskStatus }) {
  return (
    <div className="grid grid-cols-12 gap-2 px-4 py-2.5 items-center text-sm">
      <div className="col-span-3 min-w-0">
        <p className="text-[var(--color-text-primary)] truncate">{task.name}</p>
      </div>
      <div className="col-span-2 min-w-0">
        <p className="text-xs text-[var(--color-text-tertiary)] truncate">{task.id}</p>
      </div>
      <div className="col-span-2">
        <PriorityBadge priority={task.priority} />
      </div>
      <div className="col-span-2 min-w-0">
        <code className="text-xs text-[var(--color-text-secondary)] truncate">
          {task.cron_expression}
        </code>
      </div>
      <div className="col-span-1 text-xs text-[var(--color-text-secondary)]">
        {task.run_count} 次
      </div>
      <div className="col-span-2 flex items-center gap-1 min-w-0">
        <Clock size={12} className="shrink-0 text-[var(--color-text-tertiary)]" />
        <span className="text-xs text-[var(--color-text-secondary)]">
          {formatLastRun(task.last_run_ago_secs)}
        </span>
      </div>
    </div>
  );
}

export default function InsightsPanel() {
  const { data, loading, error, reload } = useResource(
    () => Promise.all([fetchSchedulerStatus(), fetchUsageStats()]),
    [],
    { errorFallback: '加载洞察数据失败' },
  );
  const scheduler = data?.[0] ?? null;
  const stats = data?.[1] ?? null;

  const tasks = scheduler?.tasks ?? [];
  const running = scheduler?.running ?? false;

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="flex items-center justify-between px-6 py-4 border-b border-[var(--color-border)]">
        <h2 className="text-lg font-semibold text-[var(--color-text-primary)]">洞察</h2>
        <button
          onClick={() => void reload()}
          disabled={loading}
          className="flex items-center gap-1 px-2 py-1 text-xs rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50"
        >
          <RefreshCw size={12} />
          刷新
        </button>
      </div>

      {/* Body */}
      <div className="flex-1 overflow-y-auto p-6 space-y-6">
        {error && <ErrorBanner message={error} onRetry={() => void reload()} />}

        {!error && loading && <Spinner />}

        {!error && !loading && scheduler && stats && (
          <>
            {/* a) 调度器状态 */}
            <section>
              <div className="flex items-center gap-2 mb-2">
                <h3 className="text-xs font-semibold text-[var(--color-text-tertiary)] uppercase tracking-wider">
                  调度器状态
                </h3>
                <RunningBadge running={running} />
              </div>

              {tasks.length === 0 ? (
                <div className="flex flex-col items-center justify-center py-10 text-[var(--color-text-tertiary)]">
                  <Gauge size={36} className="mb-2 opacity-40" />
                  <p className="text-sm">调度器未装配（无启用的模型服务时不运行定时任务）</p>
                </div>
              ) : (
                <div className="rounded-lg bg-[var(--color-bg-secondary)] border border-[var(--color-border)] overflow-hidden">
                  {/* 表头 */}
                  <div className="grid grid-cols-12 gap-2 px-4 py-2 border-b border-[var(--color-border)] text-xs text-[var(--color-text-tertiary)]">
                    <div className="col-span-3">任务名</div>
                    <div className="col-span-2">ID</div>
                    <div className="col-span-2">优先级</div>
                    <div className="col-span-2">Cron 表达式</div>
                    <div className="col-span-1">累计执行</div>
                    <div className="col-span-2">上次执行</div>
                  </div>
                  <div className="divide-y divide-[var(--color-border)]">
                    {tasks.map((task) => (
                      <TaskRow key={task.id} task={task} />
                    ))}
                  </div>
                </div>
              )}
            </section>

            {/* b) 使用统计 */}
            <section>
              <h3 className="text-xs font-semibold text-[var(--color-text-tertiary)] uppercase tracking-wider mb-2">
                使用统计
              </h3>
              <div className="grid grid-cols-2 md:grid-cols-4 gap-2">
                <div className="p-3 rounded-lg bg-[var(--color-bg-secondary)] border border-[var(--color-border)]">
                  <p className="flex items-center gap-1.5 text-xs text-[var(--color-text-tertiary)]">
                    <Wrench size={12} />
                    技能追踪数
                  </p>
                  <p className="text-xl font-semibold text-[var(--color-text-primary)] mt-1">
                    {formatNumber(stats.total_skills_tracked)}
                  </p>
                </div>
                <div className="p-3 rounded-lg bg-[var(--color-bg-secondary)] border border-[var(--color-border)]">
                  <p className="flex items-center gap-1.5 text-xs text-[var(--color-text-tertiary)]">
                    <Zap size={12} />
                    技能调用数
                  </p>
                  <p className="text-xl font-semibold text-[var(--color-text-primary)] mt-1">
                    {formatNumber(stats.total_skill_calls)}
                  </p>
                </div>
                <div className="p-3 rounded-lg bg-[var(--color-bg-secondary)] border border-[var(--color-border)]">
                  <p className="flex items-center gap-1.5 text-xs text-[var(--color-text-tertiary)]">
                    <Terminal size={12} />
                    工具追踪数
                  </p>
                  <p className="text-xl font-semibold text-[var(--color-text-primary)] mt-1">
                    {formatNumber(stats.total_tools_tracked)}
                  </p>
                </div>
                <div className="p-3 rounded-lg bg-[var(--color-bg-secondary)] border border-[var(--color-border)]">
                  <p className="flex items-center gap-1.5 text-xs text-[var(--color-text-tertiary)]">
                    <Terminal size={12} />
                    工具调用数
                  </p>
                  <p className="text-xl font-semibold text-[var(--color-text-primary)] mt-1">
                    {formatNumber(stats.total_tool_calls)}
                  </p>
                </div>
                <div className="p-3 rounded-lg bg-[var(--color-bg-secondary)] border border-[var(--color-border)]">
                  <p className="flex items-center gap-1.5 text-xs text-[var(--color-text-tertiary)]">
                    <FileText size={12} />
                    文档追踪数
                  </p>
                  <p className="text-xl font-semibold text-[var(--color-text-primary)] mt-1">
                    {formatNumber(stats.total_docs_tracked)}
                  </p>
                </div>
                <div className="p-3 rounded-lg bg-[var(--color-bg-secondary)] border border-[var(--color-border)]">
                  <p className="flex items-center gap-1.5 text-xs text-[var(--color-text-tertiary)]">
                    <Search size={12} />
                    搜索次数
                  </p>
                  <p className="text-xl font-semibold text-[var(--color-text-primary)] mt-1">
                    {formatNumber(stats.total_searches)}
                  </p>
                </div>
              </div>
            </section>
          </>
        )}
      </div>
    </div>
  );
}
