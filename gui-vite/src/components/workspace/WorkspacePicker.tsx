import { useState, useCallback, useEffect } from 'react';
import { Folder, FolderOpen, ChevronRight, ArrowUp, X, Loader2, Check } from 'lucide-react';
import { fetchWorkspaceDirs } from '@/lib/api-client';
import type { WorkspaceDirsResponse } from '@/lib/types';

interface Props {
  /** 打开状态（父组件控制）。 */
  open: boolean;
  /** 当前工作目录（用于高亮与默认展示）。 */
  currentWorkingDir: string;
  /** 关闭回调。 */
  onClose: () => void;
  /** 确认选择回调（传入选中的目录绝对路径）。 */
  onSelect: (path: string) => void;
  /** 选择中（保存等异步操作）。 */
  saving?: boolean;
  /** 提供时在底部显示「不绑定」按钮（点击回调空串，表示清除/不绑定工作区）。 */
  clearLabel?: string;
}

/**
 * 工作区目录选择器（模态）：逐级浏览任意目录，选中后回调。
 *
 * 数据源：GET /workspace/dirs（后端列出子目录；缺省 path = 浏览根：
 * Windows 盘符列表 / 其余平台家目录）。解决手动输入易输错问题——
 * 只列出真实存在的目录供点选。
 */
export default function WorkspacePicker({
  open,
  currentWorkingDir,
  onClose,
  onSelect,
  saving = false,
  clearLabel,
}: Props) {
  const [dirs, setDirs] = useState<WorkspaceDirsResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string>('');

  const load = useCallback(async (path?: string) => {
    setLoading(true);
    setError(null);
    try {
      const resp = await fetchWorkspaceDirs(path);
      setDirs(resp);
      setSelected('');
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : '目录加载失败');
    } finally {
      setLoading(false);
    }
  }, []);

  // 打开时从当前工作目录开始浏览（未配置则从根开始）
  useEffect(() => {
    if (open) {
      setSelected('');
      load(currentWorkingDir || undefined);
    }
  }, [open, currentWorkingDir, load]);

  if (!open) return null;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
      role="dialog"
      aria-modal="true"
      aria-label="选择工作目录"
      onClick={(e) => {
        if (e.target === e.currentTarget && !saving) onClose();
      }}
    >
      <div className="w-[520px] max-h-[70vh] flex flex-col rounded-xl border border-[var(--color-border)] bg-[var(--color-bg-primary)] shadow-xl">
        {/* 头部 */}
        <div className="flex items-center gap-2 px-4 py-3 border-b border-[var(--color-border)]">
          <FolderOpen className="w-4 h-4 text-accent" />
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">选择工作目录</h2>
          <button
            onClick={onClose}
            disabled={saving}
            className="ml-auto p-1 rounded-md hover:bg-[var(--color-bg-hover)] text-[var(--color-text-tertiary)]"
            aria-label="关闭"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        {/* 当前路径 + 上级 */}
        <div className="flex items-center gap-2 px-4 py-2 border-b border-[var(--color-border)]">
          <button
            onClick={() => (dirs?.parent ? load(dirs.parent) : undefined)}
            disabled={!dirs?.parent || loading || saving}
            className="flex items-center gap-1 px-2 py-1 text-xs rounded-md border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-40"
            aria-label="上级目录"
          >
            <ArrowUp className="w-3.5 h-3.5" />
            上级
          </button>
          <span
            className="flex-1 text-xs font-mono text-[var(--color-text-secondary)] truncate"
            title={dirs?.current ?? ''}
          >
            {dirs?.current ?? '...'}
          </span>
          <button
            onClick={() => load(undefined)}
            className="px-2 py-1 text-xs rounded-md border border-[var(--color-border)] text-[var(--color-text-tertiary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-40"
            disabled={loading || saving}
            aria-label="浏览根"
          >
            根
          </button>
        </div>

        {/* 目录列表 */}
        <div className="flex-1 overflow-y-auto p-2 min-h-[200px]">
          {loading ? (
            <div className="flex items-center justify-center h-full py-10" aria-label="加载中">
              <Loader2 className="w-5 h-5 animate-spin text-[var(--color-text-tertiary)]" />
            </div>
          ) : error ? (
            <p className="text-xs text-[var(--color-error)] p-3">{error}</p>
          ) : dirs && dirs.entries.length === 0 ? (
            <p className="text-xs text-[var(--color-text-tertiary)] p-3">（空目录）</p>
          ) : (
            <ul className="space-y-0.5">
              {dirs?.entries.map((entry) => {
                const isSelected = selected === entry.path;
                return (
                  <li key={entry.path}>
                    <button
                      type="button"
                      onClick={() => setSelected(entry.path)}
                      onDoubleClick={() => load(entry.path)}
                      className={`w-full flex items-center gap-2 px-2 py-1.5 rounded-md text-left transition-colors ${
                        isSelected
                          ? 'bg-accent/10 text-accent border border-accent/30'
                          : 'border border-transparent hover:bg-[var(--color-bg-hover)] text-[var(--color-text-primary)]'
                      }`}
                      aria-label={`目录 ${entry.name}`}
                    >
                      <Folder className="w-4 h-4 shrink-0 text-[var(--color-text-tertiary)]" />
                      <span className="text-sm truncate flex-1">{entry.name}</span>
                      <ChevronRight
                        className="w-3.5 h-3.5 shrink-0 text-[var(--color-text-tertiary)]"
                        onClick={(e) => {
                          e.stopPropagation();
                          load(entry.path);
                        }}
                      />
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
        </div>

        {/* 底部操作 */}
        <div className="flex items-center justify-between gap-2 px-4 py-3 border-t border-[var(--color-border)]">
          {clearLabel && (
            <button
              type="button"
              onClick={() => onSelect('')}
              disabled={saving}
              className="px-3 py-1.5 text-sm rounded-md border border-[var(--color-border)] text-[var(--color-text-tertiary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50"
              aria-label={clearLabel}
            >
              {clearLabel}
            </button>
          )}
          <div className="flex items-center gap-2">
            <button
              onClick={onClose}
              disabled={saving}
              className="px-3 py-1.5 text-sm rounded-md border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50"
            >
              取消
            </button>
            <button
              onClick={() => selected && onSelect(selected)}
              disabled={!selected || saving}
              className="flex items-center gap-1.5 px-4 py-1.5 text-sm font-medium rounded-md bg-accent text-white hover:bg-accent-hover disabled:opacity-50 transition-colors"
              aria-label="确认选择"
            >
              {saving ? (
                <Loader2 className="w-4 h-4 animate-spin" />
              ) : (
                <Check className="w-4 h-4" />
              )}
              {saving ? '保存中...' : '选择此目录'}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
