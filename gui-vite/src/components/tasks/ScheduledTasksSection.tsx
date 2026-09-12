import { useState, useCallback } from 'react';
import { usePolling } from '@/hooks/use-polling';
import { Spinner } from '@/components/ui/Spinner';
import {
  fetchScheduledTasks,
  createScheduledTask,
  deleteScheduledTask,
  type ScheduledAgentTask,
} from '@/lib/api-client';
import { formatTimestamp, formatInterval } from '@/lib/utils';
import { CalendarClock, Loader2, Plus, Trash2, X } from 'lucide-react';

/** 间隔预设（ADR-024 间隔制；自定义走秒数输入）。 */
const INTERVAL_PRESETS: { label: string; secs: number }[] = [
  { label: '每 30 分钟', secs: 1800 },
  { label: '每 1 小时', secs: 3600 },
  { label: '每 6 小时', secs: 21600 },
  { label: '每 12 小时', secs: 43200 },
  { label: '每天', secs: 86400 },
  { label: '每周', secs: 604800 },
  { label: '自定义…', secs: 0 },
];

/** 定时智能体任务：列表 + 新建 + 删除（按执行间隔调用 agent 在指定工作区工作）。 */
export default function ScheduledTasksSection() {
  const [tasks, setTasks] = useState<ScheduledAgentTask[]>([]);
  const [loading, setLoading] = useState(true);
  const [creating, setCreating] = useState(false);
  const [showForm, setShowForm] = useState(false);
  const [form, setForm] = useState({
    name: '',
    preset: 21600,
    custom: '',
    workspace: '',
    prompt: '',
  });
  const isCustom = form.preset === 0;
  const intervalSecs = isCustom ? Number(form.custom) || 0 : form.preset;

  const load = useCallback(async () => {
    try {
      const data = await fetchScheduledTasks();
      setTasks(data);
    } catch {
      /* 静默 */
    } finally {
      setLoading(false);
    }
  }, []);
  usePolling(load, 10000, { immediate: true });

  const handleCreate = async () => {
    if (!form.name.trim() || !intervalSecs || !form.prompt.trim()) return;
    setCreating(true);
    try {
      await createScheduledTask({
        name: form.name.trim(),
        interval_secs: intervalSecs,
        workspace: form.workspace.trim(),
        prompt: form.prompt.trim(),
      });
      setShowForm(false);
      setForm({ name: '', preset: 21600, custom: '', workspace: '', prompt: '' });
      await load();
    } catch {
      /* 静默 */
    } finally {
      setCreating(false);
    }
  };

  const handleDelete = async (id: string) => {
    try {
      await deleteScheduledTask(id);
      await load();
    } catch {
      /* 静默 */
    }
  };

  const field =
    'w-full px-2.5 py-1.5 text-xs rounded border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-tertiary)] focus:outline-none focus:ring-2 focus:ring-blue-500/30 focus:border-blue-500';

  return (
    <section className="space-y-2">
      <div className="flex items-center justify-between">
        <h3 className="text-xs font-semibold text-[var(--color-text-tertiary)] uppercase tracking-wider flex items-center gap-1.5">
          <CalendarClock size={14} />
          任务列表
        </h3>
        <button
          type="button"
          onClick={() => setShowForm((v) => !v)}
          className="flex items-center gap-1 px-2 py-1 text-xs rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]"
        >
          {showForm ? <X size={12} /> : <Plus size={12} />}
          {showForm ? '取消' : '新建定时任务'}
        </button>
      </div>

      {showForm && (
        <div className="space-y-2 p-3 rounded-lg bg-[var(--color-bg-secondary)] border border-[var(--color-border)]">
          <input
            value={form.name}
            onChange={(e) => setForm({ ...form, name: e.target.value })}
            placeholder="任务名称"
            className={field}
            aria-label="任务名称"
          />
          <select
            value={form.preset}
            onChange={(e) => setForm({ ...form, preset: Number(e.target.value) })}
            className={field}
            aria-label="执行间隔"
          >
            {INTERVAL_PRESETS.map((p) => (
              <option key={p.secs} value={p.secs}>
                {p.label}
              </option>
            ))}
          </select>
          {isCustom && (
            <input
              type="number"
              min={60}
              value={form.custom}
              onChange={(e) => setForm({ ...form, custom: e.target.value })}
              placeholder="自定义间隔（秒，最小 60）"
              className={field}
              aria-label="自定义间隔秒数"
            />
          )}
          <input
            value={form.workspace}
            onChange={(e) => setForm({ ...form, workspace: e.target.value })}
            placeholder="工作目录（绝对路径）"
            className={field}
            aria-label="工作目录"
          />
          <textarea
            value={form.prompt}
            onChange={(e) => setForm({ ...form, prompt: e.target.value })}
            placeholder="给智能体的指令（每次触发让它做的事）"
            rows={2}
            className={field + ' resize-y'}
            aria-label="指令"
          />
          <button
            type="button"
            onClick={() => void handleCreate()}
            disabled={creating || !form.name.trim() || !intervalSecs || !form.prompt.trim()}
            className="flex items-center justify-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded bg-blue-600 text-white hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {creating ? <Loader2 size={12} className="animate-spin" /> : <Plus size={12} />}
            创建
          </button>
        </div>
      )}

      {loading ? (
        <Spinner className="py-6" />
      ) : tasks.length === 0 ? (
        <p className="text-xs text-[var(--color-text-tertiary)] py-3 text-center">
          暂无定时任务——可让智能体用 schedule_task 工具创建，或在此手动创建
        </p>
      ) : (
        <div className="space-y-2">
          {tasks.map((t) => (
            <div
              key={t.id}
              className="p-3 rounded-lg bg-[var(--color-bg-secondary)] border border-[var(--color-border)]"
            >
              <div className="flex items-start justify-between gap-2">
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="text-sm font-medium text-[var(--color-text-primary)] truncate">
                      {t.name}
                    </span>
                    <span className="px-1.5 py-0.5 text-[10px] rounded bg-blue-50 dark:bg-blue-900/30 text-blue-700 dark:text-blue-300 border border-blue-200 dark:border-blue-800">
                      {formatInterval(t.interval_secs)}
                    </span>
                  </div>
                  <p
                    className="text-xs text-[var(--color-text-tertiary)] truncate mt-0.5"
                    title={t.workspace}
                  >
                    工作区: {t.workspace || '(默认)'}
                  </p>
                  <p className="text-xs text-[var(--color-text-secondary)] mt-0.5 line-clamp-2">
                    {t.prompt}
                  </p>
                  <div className="flex items-center gap-2 mt-1 text-[10px] text-[var(--color-text-tertiary)]">
                    <span>下次(约): {t.next_run_at ? formatTimestamp(t.next_run_at) : '待定'}</span>
                    {t.last_run_at !== null && <span>上次: {formatTimestamp(t.last_run_at)}</span>}
                  </div>
                  {t.last_result && (
                    <p className="text-xs text-[var(--color-text-tertiary)] mt-1 break-words line-clamp-2">
                      结果: {t.last_result}
                    </p>
                  )}
                </div>
                <button
                  type="button"
                  onClick={() => void handleDelete(t.id)}
                  aria-label={'删除定时任务 ' + t.name}
                  className="p-1 rounded hover:bg-red-100 dark:hover:bg-red-900/30 text-[var(--color-text-tertiary)] hover:text-red-500 shrink-0"
                >
                  <Trash2 size={14} />
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}
