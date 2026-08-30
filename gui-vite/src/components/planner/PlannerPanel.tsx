import { useCallback, useState } from 'react';
import { usePolling } from '@/hooks/use-polling';
import { Spinner } from '@/components/ui/Spinner';
import { EmptyState } from '@/components/ui/EmptyState';
import {
  fetchTodos,
  createTodo,
  updateTodo,
  deleteTodo,
  fetchGoals,
  createGoal,
  updateGoal,
  deleteGoal,
} from '@/lib/api-client';
import { formatTimestamp } from '@/lib/utils';
import type { Goal, GoalWithProgress, TodoItem, TodoPriority, TodoStatus } from '@/lib/types';
import {
  CheckSquare,
  Target,
  Plus,
  Trash2,
  Loader2,
  RefreshCw,
  Circle,
  CircleCheck,
  Flag,
} from 'lucide-react';

const POLL_INTERVAL_MS = 5000;

/* ───────── 待办 tab ───────── */

const PRIORITY_LABELS: Record<TodoPriority, string> = {
  low: '低',
  medium: '中',
  high: '高',
};

const PRIORITY_CLASSES: Record<TodoPriority, string> = {
  low: 'bg-gray-50 dark:bg-gray-900/30 text-gray-500 dark:text-gray-400 border-gray-200 dark:border-gray-700',
  medium: 'bg-blue-50 dark:bg-blue-900/30 text-blue-600 dark:text-blue-300 border-blue-200 dark:border-blue-800',
  high: 'bg-red-50 dark:bg-red-900/30 text-red-600 dark:text-red-300 border-red-200 dark:border-red-800',
};

const STATUS_FILTERS: { id: 'all' | 'active' | 'completed'; label: string }[] = [
  { id: 'all', label: '全部' },
  { id: 'active', label: '未完成' },
  { id: 'completed', label: '已完成' },
];

function TodosTab({ goals }: { goals: Goal[] }) {
  const [todos, setTodos] = useState<TodoItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [filter, setFilter] = useState<'all' | 'active' | 'completed'>('all');
  const [showForm, setShowForm] = useState(false);
  const [form, setForm] = useState({ title: '', priority: 'medium' as TodoPriority, goal_id: '' });
  const [creating, setCreating] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editTitle, setEditTitle] = useState('');

  const load = useCallback(async () => {
    try {
      const data = await fetchTodos();
      setTodos(data.todos);
    } catch {
      /* 静默 */
    } finally {
      setLoading(false);
    }
  }, []);
  usePolling(load, POLL_INTERVAL_MS, { immediate: true });

  const handleCreate = async () => {
    if (!form.title.trim()) return;
    setCreating(true);
    try {
      await createTodo({
        title: form.title.trim(),
        priority: form.priority,
        ...(form.goal_id ? { goal_id: form.goal_id } : {}),
      });
      setForm({ title: '', priority: 'medium', goal_id: '' });
      setShowForm(false);
      await load();
    } catch {
      /* 静默 */
    } finally {
      setCreating(false);
    }
  };

  const handleToggle = async (todo: TodoItem) => {
    const next: TodoStatus = todo.status === 'completed' ? 'pending' : 'completed';
    try {
      await updateTodo(todo.id, { status: next });
      await load();
    } catch {
      /* 静默 */
    }
  };

  const handlePriority = async (todo: TodoItem, priority: TodoPriority) => {
    try {
      await updateTodo(todo.id, { priority });
      await load();
    } catch {
      /* 静默 */
    }
  };

  const handleGoal = async (todo: TodoItem, goal_id: string) => {
    try {
      await updateTodo(todo.id, { goal_id });
      await load();
    } catch {
      /* 静默 */
    }
  };

  const handleDelete = async (id: string) => {
    try {
      await deleteTodo(id);
      await load();
    } catch {
      /* 静默 */
    }
  };

  const handleEditSave = async (id: string) => {
    const title = editTitle.trim();
    if (!title) {
      setEditingId(null);
      return;
    }
    try {
      await updateTodo(id, { title });
      setEditingId(null);
      await load();
    } catch {
      /* 静默 */
    }
  };

  const visible = todos.filter((t) =>
    filter === 'all' ? true : filter === 'completed' ? t.status === 'completed' : t.status !== 'completed',
  );
  const activeCount = todos.filter((t) => t.status !== 'completed').length;

  const field =
    'w-full px-2.5 py-1.5 text-xs rounded border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-tertiary)] focus:outline-none focus:ring-2 focus:ring-blue-500/30 focus:border-blue-500';

  return (
    <div className="space-y-3">
      {/* 工具行：筛选 + 新建 */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-1">
          {STATUS_FILTERS.map((f) => (
            <button
              key={f.id}
              onClick={() => setFilter(f.id)}
              className={`px-2 py-1 text-xs rounded border transition-colors ${
                filter === f.id
                  ? 'border-accent text-accent bg-accent/5 font-medium'
                  : 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]'
              }`}
            >
              {f.label}
            </button>
          ))}
        </div>
        <button
          onClick={() => setShowForm((v) => !v)}
          className="flex items-center gap-1 px-2.5 py-1 text-xs rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]"
        >
          {showForm ? '取消' : <Plus size={12} />}
          {showForm ? '取消' : '新建待办'}
        </button>
      </div>

      {/* 新建表单 */}
      {showForm && (
        <div className="space-y-2 p-3 rounded-lg bg-[var(--color-bg-secondary)] border border-[var(--color-border)]">
          <input
            value={form.title}
            onChange={(e) => setForm({ ...form, title: e.target.value })}
            placeholder="要做什么？"
            className={field}
            aria-label="待办标题"
            onKeyDown={(e) => e.key === 'Enter' && void handleCreate()}
          />
          <div className="flex items-center gap-2">
            <select
              value={form.priority}
              onChange={(e) => setForm({ ...form, priority: e.target.value as TodoPriority })}
              className={field + ' w-24 shrink-0'}
              aria-label="优先级"
            >
              <option value="low">低优先级</option>
              <option value="medium">中优先级</option>
              <option value="high">高优先级</option>
            </select>
            {goals.length > 0 && (
              <select
                value={form.goal_id}
                onChange={(e) => setForm({ ...form, goal_id: e.target.value })}
                className={field + ' flex-1'}
                aria-label="关联目标"
              >
                <option value="">不关联目标</option>
                {goals.map((g) => (
                  <option key={g.id} value={g.id}>
                    {g.title}
                  </option>
                ))}
              </select>
            )}
            <button
              onClick={() => void handleCreate()}
              disabled={creating || !form.title.trim()}
              className="flex items-center justify-center gap-1 px-3 py-1.5 text-xs font-medium rounded bg-blue-600 text-white hover:bg-blue-700 disabled:opacity-50 shrink-0"
            >
              {creating ? <Loader2 size={12} className="animate-spin" /> : <Plus size={12} />}
              添加
            </button>
          </div>
        </div>
      )}

      {/* 统计行 */}
      <p className="text-xs text-[var(--color-text-tertiary)]">
        {activeCount} 项未完成 / 共 {todos.length} 项
      </p>

      {loading ? (
        <Spinner className="py-6" />
      ) : visible.length === 0 ? (
        <EmptyState icon={CheckSquare} title={filter === 'completed' ? '暂无已完成待办' : '暂无待办'} hint="新建待办跟踪多步任务，可关联目标自动累计进度" />
      ) : (
        <div className="space-y-1.5">
          {visible.map((todo) => {
            const done = todo.status === 'completed';
            const goal = goals.find((g) => g.id === todo.goal_id);
            return (
              <div
                key={todo.id}
                className={`p-3 rounded-lg bg-[var(--color-bg-secondary)] border border-[var(--color-border)] flex items-start gap-2.5 ${
                  done ? 'opacity-60' : ''
                }`}
              >
                {/* 完成切换 */}
                <button
                  onClick={() => void handleToggle(todo)}
                  aria-label={done ? '标记未完成' : '标记完成'}
                  className="mt-0.5 shrink-0 text-[var(--color-text-tertiary)] hover:text-accent"
                >
                  {done ? <CircleCheck size={18} className="text-green-500" /> : <Circle size={18} />}
                </button>

                <div className="min-w-0 flex-1">
                  {editingId === todo.id ? (
                    <input
                      autoFocus
                      value={editTitle}
                      onChange={(e) => setEditTitle(e.target.value)}
                      onBlur={() => void handleEditSave(todo.id)}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter') void handleEditSave(todo.id);
                        if (e.key === 'Escape') setEditingId(null);
                      }}
                      className={field}
                      aria-label="编辑标题"
                    />
                  ) : (
                    <p
                      className={`text-sm text-[var(--color-text-primary)] break-all cursor-text ${
                        done ? 'line-through' : ''
                      }`}
                      onDoubleClick={() => {
                        setEditingId(todo.id);
                        setEditTitle(todo.title);
                      }}
                      title="双击编辑"
                    >
                      {todo.title}
                    </p>
                  )}
                  {todo.description && !done && (
                    <p className="text-xs text-[var(--color-text-tertiary)] mt-0.5 break-all">
                      {todo.description}
                    </p>
                  )}
                  <div className="flex items-center gap-2 mt-1.5 flex-wrap">
                    <span className={`px-1.5 py-0.5 text-[10px] rounded border ${PRIORITY_CLASSES[todo.priority]}`}>
                      {PRIORITY_LABELS[todo.priority]}
                    </span>
                    {goal && (
                      <span className="px-1.5 py-0.5 text-[10px] rounded border border-purple-200 dark:border-purple-800 bg-purple-50 dark:bg-purple-900/30 text-purple-600 dark:text-purple-300 flex items-center gap-1">
                        <Target size={10} />
                        {goal.title}
                      </span>
                    )}
                    {todo.due_at !== null && (
                      <span className="text-[10px] text-[var(--color-text-tertiary)] flex items-center gap-1">
                        <Flag size={10} />
                        {formatTimestamp(todo.due_at * 1000)}
                      </span>
                    )}
                    <span className="text-[10px] text-[var(--color-text-tertiary)]">
                      {formatTimestamp(todo.created_at * 1000)}
                    </span>
                  </div>
                </div>

                {/* 操作列 */}
                <div className="flex items-center gap-1 shrink-0">
                  <select
                    value={todo.priority}
                    onChange={(e) => void handlePriority(todo, e.target.value as TodoPriority)}
                    className="px-1 py-0.5 text-[10px] rounded border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-secondary)] focus:outline-none"
                    aria-label="修改优先级"
                  >
                    <option value="low">低</option>
                    <option value="medium">中</option>
                    <option value="high">高</option>
                  </select>
                  {goals.length > 0 && (
                    <select
                      value={todo.goal_id ?? ''}
                      onChange={(e) => void handleGoal(todo, e.target.value)}
                      className="px-1 py-0.5 text-[10px] rounded border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-secondary)] focus:outline-none max-w-24"
                      aria-label="关联目标"
                    >
                      <option value="">无目标</option>
                      {goals.map((g) => (
                        <option key={g.id} value={g.id}>
                          {g.title}
                        </option>
                      ))}
                    </select>
                  )}
                  <button
                    onClick={() => void handleDelete(todo.id)}
                    aria-label={`删除待办 ${todo.title}`}
                    className="p-1 rounded hover:bg-red-100 dark:hover:bg-red-900/30 text-[var(--color-text-tertiary)] hover:text-red-500"
                  >
                    <Trash2 size={14} />
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

/* ───────── 目标 tab ───────── */

function GoalsTab() {
  const [goals, setGoals] = useState<GoalWithProgress[]>([]);
  const [loading, setLoading] = useState(true);
  const [showForm, setShowForm] = useState(false);
  const [form, setForm] = useState({ title: '', description: '' });
  const [creating, setCreating] = useState(false);

  const load = useCallback(async () => {
    try {
      const data = await fetchGoals();
      setGoals(data.goals);
    } catch {
      /* 静默 */
    } finally {
      setLoading(false);
    }
  }, []);
  usePolling(load, POLL_INTERVAL_MS, { immediate: true });

  const handleCreate = async () => {
    if (!form.title.trim()) return;
    setCreating(true);
    try {
      await createGoal({
        title: form.title.trim(),
        ...(form.description.trim() ? { description: form.description.trim() } : {}),
      });
      setForm({ title: '', description: '' });
      setShowForm(false);
      await load();
    } catch {
      /* 静默 */
    } finally {
      setCreating(false);
    }
  };

  const handleStatus = async (goal: Goal, status: string) => {
    try {
      await updateGoal(goal.id, { status: status as Goal['status'] });
      await load();
    } catch {
      /* 静默 */
    }
  };

  const handleDelete = async (id: string) => {
    try {
      await deleteGoal(id);
      await load();
    } catch {
      /* 静默 */
    }
  };

  const field =
    'w-full px-2.5 py-1.5 text-xs rounded border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-tertiary)] focus:outline-none focus:ring-2 focus:ring-blue-500/30 focus:border-blue-500';

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between">
        <p className="text-xs text-[var(--color-text-tertiary)]">
          长期目标 + 进度跟踪；进度按关联待办完成比例自动计算
        </p>
        <button
          onClick={() => setShowForm((v) => !v)}
          className="flex items-center gap-1 px-2.5 py-1 text-xs rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]"
        >
          {showForm ? '取消' : <Plus size={12} />}
          {showForm ? '取消' : '新建目标'}
        </button>
      </div>

      {showForm && (
        <div className="space-y-2 p-3 rounded-lg bg-[var(--color-bg-secondary)] border border-[var(--color-border)]">
          <input
            value={form.title}
            onChange={(e) => setForm({ ...form, title: e.target.value })}
            placeholder="目标名称（如：学会 Rust）"
            className={field}
            aria-label="目标名称"
            onKeyDown={(e) => e.key === 'Enter' && void handleCreate()}
          />
          <input
            value={form.description}
            onChange={(e) => setForm({ ...form, description: e.target.value })}
            placeholder="目标描述（可选）"
            className={field}
            aria-label="目标描述"
          />
          <button
            onClick={() => void handleCreate()}
            disabled={creating || !form.title.trim()}
            className="flex items-center justify-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded bg-blue-600 text-white hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {creating ? <Loader2 size={12} className="animate-spin" /> : <Plus size={12} />}
            创建
          </button>
        </div>
      )}

      {loading ? (
        <Spinner className="py-6" />
      ) : goals.length === 0 ? (
        <EmptyState icon={Target} title="暂无目标" hint="创建长期目标，再把待办关联到目标上自动累计进度" />
      ) : (
        <div className="space-y-2">
          {goals.map(({ goal, progress, todo_total, todo_done }) => {
            const done = goal.status === 'completed';
            return (
              <div
                key={goal.id}
                className={`p-4 rounded-lg bg-[var(--color-bg-secondary)] border border-[var(--color-border)] ${
                  done ? 'opacity-60' : ''
                }`}
              >
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <Target size={15} className="shrink-0 text-purple-500" />
                      <p className="text-sm font-medium text-[var(--color-text-primary)] break-all">
                        {goal.title}
                      </p>
                    </div>
                    {goal.description && (
                      <p className="text-xs text-[var(--color-text-tertiary)] mt-1 break-all">
                        {goal.description}
                      </p>
                    )}
                    <div className="flex items-center gap-2 mt-2">
                      <div className="flex-1 h-1.5 rounded-full bg-[var(--color-bg-tertiary)] overflow-hidden">
                        <div
                          className={`h-full rounded-full transition-all ${
                            progress >= 100 ? 'bg-green-500' : 'bg-accent'
                          }`}
                          style={{ width: `${progress}%` }}
                        />
                      </div>
                      <span className="text-xs text-[var(--color-text-secondary)] tabular-nums shrink-0">
                        {progress}%
                      </span>
                    </div>
                    <div className="flex items-center gap-2 mt-1.5 text-[10px] text-[var(--color-text-tertiary)]">
                      <span>
                        待办 {todo_done}/{todo_total}
                      </span>
                      <span>{formatTimestamp(goal.created_at * 1000)}</span>
                      {goal.target_date !== null && <span>目标日 {formatTimestamp(goal.target_date * 1000)}</span>}
                    </div>
                  </div>
                  <div className="flex items-center gap-1 shrink-0">
                    <select
                      value={goal.status}
                      onChange={(e) => void handleStatus(goal, e.target.value)}
                      className="px-1 py-0.5 text-[10px] rounded border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-secondary)] focus:outline-none"
                      aria-label="目标状态"
                    >
                      <option value="active">进行中</option>
                      <option value="completed">已完成</option>
                      <option value="archived">已归档</option>
                    </select>
                    <button
                      onClick={() => void handleDelete(goal.id)}
                      aria-label={`删除目标 ${goal.title}`}
                      className="p-1 rounded hover:bg-red-100 dark:hover:bg-red-900/30 text-[var(--color-text-tertiary)] hover:text-red-500"
                    >
                      <Trash2 size={14} />
                    </button>
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

/* ───────── 面板主体 ───────── */

type PlannerTab = 'todos' | 'goals';

export default function PlannerPanel() {
  const [tab, setTab] = useState<PlannerTab>('todos');
  const [goals, setGoals] = useState<Goal[]>([]);
  const [refreshing, setRefreshing] = useState(false);

  // 目标列表供待办 tab 关联选择（独立轮询，与 GoalsTab 各自加载）
  const loadGoals = useCallback(async () => {
    try {
      const data = await fetchGoals();
      setGoals(data.goals.map((g) => g.goal));
    } catch {
      /* 静默 */
    }
  }, []);
  usePolling(loadGoals, POLL_INTERVAL_MS, { immediate: true });

  const handleRefresh = async () => {
    setRefreshing(true);
    try {
      await Promise.all([loadGoals()]);
    } finally {
      setRefreshing(false);
    }
  };

  const TABS: { id: PlannerTab; label: string; icon: typeof CheckSquare }[] = [
    { id: 'todos', label: '待办清单', icon: CheckSquare },
    { id: 'goals', label: '目标', icon: Target },
  ];

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="flex items-center justify-between px-6 py-4 border-b border-[var(--color-border)]">
        <h2 className="text-lg font-semibold text-[var(--color-text-primary)]">计划</h2>
        <button
          onClick={() => void handleRefresh()}
          disabled={refreshing}
          className="flex items-center gap-1 px-2 py-1 text-xs rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50"
        >
          <RefreshCw size={12} />
          刷新
        </button>
      </div>

      {/* Tab bar */}
      <div
        role="tablist"
        aria-label="计划分类"
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
        {tab === 'todos' ? <TodosTab goals={goals} /> : <GoalsTab />}
      </div>
    </div>
  );
}
