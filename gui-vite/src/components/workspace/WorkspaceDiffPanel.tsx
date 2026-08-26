import { useCallback, useEffect, useState } from 'react';
import { AlertCircle, GitCompareArrows, Loader2, PanelRightClose } from 'lucide-react';
import { fetchWorkspaceDiff, fetchWorkspaceDiffList } from '@/lib/api-client';
import { toErrorMessage } from '@/lib/errors';
import type { WorkspaceDiffListResponse, WorkspaceDiffResponse } from '@/lib/types';
import { useAppStore } from '@/lib/store';

const DIFF_STATUS_LABEL: Record<WorkspaceDiffResponse['status'], string> = {
  modified: '已修改',
  added: '新增',
  removed: '已删除',
  binary: '二进制',
  unchanged: '未变更',
};

/**
 * 渲染 unified diff 文本的每一行，按前缀着色：
 * '@@' 头 → 蓝色；'+' 新增 → 绿色；'-' 删除 → 红色；文件头/上下文 → 次要色。
 *
 * Phase 1 决策：@codemirror/merge 的 MergeView 需要 old/new 两个 EditorState，
 * 而 API 只返回 unified 文本（无两侧内容），无法直接喂给 MergeView。
 * 因此 Phase 1 用带 +/- 行着色的 <pre> 渲染 unified 文本（依赖轻、正确性直观），
 * @codemirror/merge 集成留待 Phase 2（API 返回 old/new 内容时）。
 */
function DiffLine({ line }: { line: string }) {
  let className = 'text-[var(--color-text-secondary)]';
  if (line.startsWith('@@')) {
    className = 'text-blue-600 dark:text-blue-400 font-medium';
  } else if (line.startsWith('--- ') || line.startsWith('+++ ')) {
    className = 'text-[var(--color-text-tertiary)]';
  } else if (line.startsWith('+')) {
    className = 'text-green-600 dark:text-green-400';
  } else if (line.startsWith('-')) {
    className = 'text-red-600 dark:text-red-400';
  }
  return <div className={className}>{line}</div>;
}

interface DiffPanelProps {
  filePath: string | null;
  onClose: () => void;
}

/** 右侧 diff 面板：快照选择器 + unified diff 渲染（默认隐藏，由父级控制）。 */
export default function DiffPanel({ filePath, onClose }: DiffPanelProps) {
  // 快照 diff 后端要求 base=snapshot 时必须携带 session_id（缺失 → 400），
  // 因此会话 ID 为必填：从 store 预填当前会话（可编辑），为空时阻止请求。
  const currentSessionId = useAppStore((s) => s.currentSessionId);
  const [sessionId, setSessionId] = useState(currentSessionId ?? '');
  // 当前会话切换（侧边栏点击）后同步到输入框 —— useState 初值只在挂载时读取。
  useEffect(() => {
    setSessionId(currentSessionId ?? '');
  }, [currentSessionId]);
  const [sessionIdError, setSessionIdError] = useState<string | null>(null);
  const [indexText, setIndexText] = useState('0');
  /** 单文件 diff 视图（文件 Diff 模式）。 */
  const [diff, setDiff] = useState<WorkspaceDiffResponse | null>(null);
  /** 工作区整体 diff 文件列表（整体差异模式）。 */
  const [diffList, setDiffList] = useState<WorkspaceDiffListResponse | null>(null);
  const [mode, setMode] = useState<'file' | 'workspace'>('file');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  /** 统一加载信封（F4）：setLoading/setError/try-catch-finally 唯一定义点。 */
  const runLoad = useCallback(async (task: () => Promise<void>, fallback: string) => {
    setLoading(true);
    setError(null);
    try {
      await task();
    } catch (err: unknown) {
      setError(toErrorMessage(err, fallback));
    } finally {
      setLoading(false);
    }
  }, []);

  /** 会话 ID 校验（三个入口共用）：空 → 置错并返回 null。 */
  const requireSession = useCallback((): string | null => {
    const trimmed = sessionId.trim();
    if (!trimmed) {
      setSessionIdError('请填写会话 ID');
      return null;
    }
    setSessionIdError(null);
    return trimmed;
  }, [sessionId]);

  const loadFileDiff = async (path: string, session: string) => {
    const indexNum = Number(indexText);
    await runLoad(async () => {
      setDiff(null);
      const res = await fetchWorkspaceDiff(
        path,
        session,
        Number.isFinite(indexNum) ? indexNum : undefined,
      );
      setDiff(res);
    }, '加载 diff 失败');
  };

  const handleLoad = async () => {
    if (!filePath) return;
    const session = requireSession();
    if (session === null) return;
    await loadFileDiff(filePath, session);
  };

  // 自动加载：选中文件 + 存在当前会话 → 立即拉取该文件 diff（P0：去掉手工
  // "填写会话 ID + 点加载" 的摩擦；文件或当前会话变化自动重取）。
  // 用 store 的 currentSessionId（而非可编辑输入框）：避免输入框逐字符触发请求；
  // 手动改会话 ID 走「加载 diff」按钮（高级用：对比其他会话）。
  useEffect(() => {
    if (!filePath || !currentSessionId) return;
    loadFileDiff(filePath, currentSessionId);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filePath, currentSessionId]);

  /** 切到整体差异：无 path 调用后端，返回工作区全部变更文件。 */
  const handleWorkspaceMode = async () => {
    const session = requireSession();
    if (session === null) return;
    setMode('workspace');
    const indexNum = Number(indexText);
    setDiffList(null);
    await runLoad(async () => {
      setDiff(null);
      const res = await fetchWorkspaceDiffList(
        session,
        Number.isFinite(indexNum) ? indexNum : undefined,
      );
      setDiffList(res);
    }, '加载整体 diff 失败');
  };

  /** 点击整体差异列表中的文件行 → 切回单文件模式并加载该文件 diff。 */
  const selectFileFromList = async (item: WorkspaceDiffResponse) => {
    if (!item.path) return;
    const session = requireSession();
    if (session === null) return;
    setMode('file');
    await loadFileDiff(item.path, session);
  };

  const toggleButtonClass = (active: boolean) =>
    `flex-1 px-2 py-1 text-xs rounded transition-colors ${
      active
        ? 'bg-blue-600 text-white'
        : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]'
    }`;

  return (
    <aside
      aria-label="diff 面板"
      className="w-96 shrink-0 flex flex-col border-l border-[var(--color-border)] bg-[var(--color-bg-primary)] min-h-0"
    >
      <div className="flex items-center justify-between px-4 py-2.5 border-b border-[var(--color-border)]">
        <h3 className="text-sm font-medium text-[var(--color-text-primary)]">快照 Diff</h3>
        <button
          type="button"
          onClick={onClose}
          aria-label="收起 diff 面板"
          className="p-1 rounded hover:bg-[var(--color-bg-hover)] text-[var(--color-text-secondary)]"
        >
          <PanelRightClose size={16} />
        </button>
      </div>

      <div className="px-4 py-3 space-y-2.5 border-b border-[var(--color-border)] bg-[var(--color-bg-secondary)]">
        <div
          role="group"
          aria-label="diff 模式"
          className="flex gap-1 p-0.5 rounded bg-[var(--color-bg-primary)] border border-[var(--color-border)]"
        >
          <button
            type="button"
            aria-pressed={mode === 'file'}
            onClick={() => setMode('file')}
            className={toggleButtonClass(mode === 'file')}
          >
            文件 Diff
          </button>
          <button
            type="button"
            aria-pressed={mode === 'workspace'}
            onClick={handleWorkspaceMode}
            className={toggleButtonClass(mode === 'workspace')}
          >
            整体差异
          </button>
        </div>
        <label className="block">
          <span className="block text-xs text-[var(--color-text-tertiary)] mb-1">
            会话（当前会话）
          </span>
          <input
            type="text"
            value={sessionId}
            onChange={(e) => {
              setSessionId(e.target.value);
              if (sessionIdError) setSessionIdError(null);
            }}
            aria-label="会话 ID"
            placeholder="会话 ID"
            aria-invalid={sessionIdError !== null}
            className="w-full px-2.5 py-1.5 text-xs rounded border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-tertiary)] focus:outline-none focus:ring-2 focus:ring-blue-500/30 focus:border-blue-500"
          />
        </label>
        {sessionIdError && (
          <p role="alert" className="text-xs text-red-600 dark:text-red-400">
            {sessionIdError}
          </p>
        )}
        <label className="block">
          <span className="block text-xs text-[var(--color-text-tertiary)] mb-1">快照索引</span>
          <input
            type="number"
            value={indexText}
            onChange={(e) => setIndexText(e.target.value)}
            aria-label="快照索引"
            min={0}
            className="w-full px-2.5 py-1.5 text-xs rounded border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-2 focus:ring-blue-500/30 focus:border-blue-500"
          />
        </label>
        <button
          type="button"
          onClick={mode === 'file' ? handleLoad : handleWorkspaceMode}
          disabled={loading || (mode === 'file' && !filePath)}
          className="w-full flex items-center justify-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded bg-blue-600 text-white hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {loading ? (
            <Loader2 size={12} className="animate-spin" />
          ) : (
            <GitCompareArrows size={12} />
          )}
          {mode === 'file' ? '加载 diff' : '加载整体 diff'}
        </button>
        {mode === 'file' && !filePath && (
          <p className="text-xs text-[var(--color-text-tertiary)]">请先在左侧选择一个文件</p>
        )}
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto">
        {error && (
          <div
            role="alert"
            className="flex items-center gap-2 m-3 p-2.5 rounded-lg bg-red-50 dark:bg-red-950 border border-red-200 dark:border-red-800 text-red-700 dark:text-red-300 text-xs"
          >
            <AlertCircle size={14} />
            <span>加载 diff 失败：{error}</span>
          </div>
        )}
        {mode === 'workspace'
          ? diffList && (
              <ul className="p-2 space-y-1">
                {diffList.files.map((item) => (
                  <li key={item.path ?? item.unified}>
                    <button
                      type="button"
                      onClick={() => selectFileFromList(item)}
                      className="w-full flex items-center justify-between gap-2 px-2.5 py-2 rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] hover:bg-[var(--color-bg-hover)] text-left"
                    >
                      <span className="text-xs font-mono text-[var(--color-text-primary)] truncate">
                        {item.path}
                      </span>
                      <span className="flex items-center gap-2 shrink-0">
                        {item.old_lines !== undefined && item.new_lines !== undefined && (
                          <span className="text-[10px] text-[var(--color-text-tertiary)]">
                            {item.old_lines} → {item.new_lines} 行
                          </span>
                        )}
                        <span className="text-xs px-1.5 py-0.5 rounded bg-blue-50 dark:bg-blue-900/30 text-blue-700 dark:text-blue-300 border border-blue-200 dark:border-blue-800">
                          {DIFF_STATUS_LABEL[item.status]}
                        </span>
                      </span>
                    </button>
                  </li>
                ))}
              </ul>
            )
          : diff && (
              <div className="p-3 space-y-2">
                <div className="flex items-center gap-2">
                  <span className="text-xs px-1.5 py-0.5 rounded bg-blue-50 dark:bg-blue-900/30 text-blue-700 dark:text-blue-300 border border-blue-200 dark:border-blue-800">
                    {DIFF_STATUS_LABEL[diff.status]}
                  </span>
                  {diff.old_lines !== undefined && diff.new_lines !== undefined && (
                    <span className="text-xs text-[var(--color-text-tertiary)]">
                      {diff.old_lines} → {diff.new_lines} 行
                    </span>
                  )}
                </div>
                <pre className="p-3 rounded-lg bg-[var(--color-bg-secondary)] border border-[var(--color-border)] font-mono text-xs leading-relaxed overflow-x-auto">
                  {(diff.unified ?? '').split('\n').map((line, i) => (
                    <DiffLine key={`${i}-${line}`} line={line} />
                  ))}
                </pre>
              </div>
            )}
      </div>
    </aside>
  );
}
