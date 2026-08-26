import { useEffect, useRef, useState } from 'react';
import {
  fetchKnowledgeEntries,
  fetchKnowledgeEntryContent,
  deleteKnowledgeEntry,
  type KnowledgeEntryItem,
} from '@/lib/api-client';
import { useResource } from '@/hooks/use-resource';
import ConfirmDialog from '@/components/ui/ConfirmDialog';
import { toErrorMessage } from '@/lib/errors';
import { ChevronRight, Folder, File, Trash2, Loader2, AlertCircle } from 'lucide-react';

/** 知识库浏览（目录导航 + 层级查看 + 二次确认删除）。 */
export default function KnowledgeBrowseTab({ active = true }: { active?: boolean }) {
  const [selectedBrowseEntry, setSelectedBrowseEntry] = useState<KnowledgeEntryItem | null>(null);
  const [browseContent, setBrowseContent] = useState('');
  const [browseLevel, setBrowseLevel] = useState('detail');
  const [browseContentLoading, setBrowseContentLoading] = useState(false);
  const [browsePath, setBrowsePath] = useState<string[]>([]);
  // Browse delete state（确认走统一 ConfirmDialog 原语——E3：对齐 RolesPanel/McpTab）
  const [confirmTarget, setConfirmTarget] = useState<{
    uri: string;
    name: string;
    isDirectory: boolean;
  } | null>(null);
  const [deletingUri, setDeletingUri] = useState<string | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);

  // 确认框 5 秒未操作自动取消（保留原内联确认的自动消失语义）
  useEffect(() => {
    if (!confirmTarget) return;
    const timer = setTimeout(() => setConfirmTarget(null), 5000);
    return () => clearTimeout(timer);
  }, [confirmTarget]);

  // 目录列表加载（useResource：路径变化自动重取 + 加载/错误/reload 收敛）
  const {
    data: browseData,
    loading: browseLoading,
    error: browseError,
    reload: reloadBrowse,
  } = useResource(
    async () => {
      setDeleteError(null);
      return fetchKnowledgeEntries(browsePath.join('/') || undefined);
    },
    [browsePath],
    { errorFallback: '加载失败' },
  );
  const browseEntries = browseData?.entries ?? [];

  // tab 保持挂载（状态保留），但列表必须新鲜：非激活时静默；
  // 从非激活切换为激活时重新拉取（否则导入新条目后切回浏览仍显示旧列表）。
  const prevActiveRef = useRef(active);
  useEffect(() => {
    if (active && !prevActiveRef.current) reloadBrowse();
    prevActiveRef.current = active;
  }, [active, reloadBrowse]);

  const handleBrowseView = async (entry: KnowledgeEntryItem) => {
    // VFS 中已 ingest 的文档以目录节点形态列出（is_directory=true 但携带 L0/L1/L2 内容）。
    // 只要条目带内容层级就按文件读取；仅纯容器目录（无内容层级）才导航进入。
    const hasContent = entry.has_abstract || entry.has_overview || entry.has_detail;
    if (entry.is_directory && !hasContent) {
      // Navigate into subdirectory
      setBrowsePath((prev) => [...prev, entry.name]);
      setSelectedBrowseEntry(null);
      return;
    }
    setSelectedBrowseEntry(entry);
    setBrowseContentLoading(true);
    try {
      const res = await fetchKnowledgeEntryContent(entry.uri, browseLevel);
      setBrowseContent(res.content);
    } catch (err: unknown) {
      setBrowseContent(`加载失败: ${toErrorMessage(err, '未知错误')}`);
    } finally {
      setBrowseContentLoading(false);
    }
  };

  // 查看层级切换时重取当前选中条目的内容（P0：否则显示不变，"点了没反应"）。
  // prevLevelRef 保证只在层级真正变化时触发（选中条目不触发重复请求）。
  const prevLevelRef = useRef(browseLevel);
  useEffect(() => {
    const entry = selectedBrowseEntry;
    if (prevLevelRef.current === browseLevel || !entry) return;
    prevLevelRef.current = browseLevel;
    if (entry.is_directory && !(entry.has_abstract || entry.has_overview || entry.has_detail)) {
      return; // 纯容器目录无内容层级，无需重取
    }
    let cancelled = false;
    setBrowseContentLoading(true);
    fetchKnowledgeEntryContent(entry.uri, browseLevel)
      .then((res) => {
        if (!cancelled) setBrowseContent(res.content);
      })
      .catch((err: unknown) => {
        if (!cancelled) setBrowseContent(`加载失败: ${toErrorMessage(err, '未知错误')}`);
      })
      .finally(() => {
        if (!cancelled) setBrowseContentLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [browseLevel, selectedBrowseEntry]);

  const handleDeleteClick = (entry: KnowledgeEntryItem) => {
    setDeleteError(null);
    setConfirmTarget({ uri: entry.uri, name: entry.name, isDirectory: entry.is_directory });
  };

  const handleConfirmDelete = async (entry: { uri: string }) => {
    setConfirmTarget(null);
    setDeletingUri(entry.uri);
    setDeleteError(null);
    try {
      const res = await deleteKnowledgeEntry(entry.uri);
      if (!res.success) {
        setDeleteError('删除失败');
        return;
      }
      // 删除的是展开查看中的条目则收起；目录被删则收起其子条目
      setSelectedBrowseEntry((prev) => {
        if (!prev) return null;
        if (prev.uri === entry.uri || prev.uri.startsWith(`${entry.uri}/`)) return null;
        return prev;
      });
      reloadBrowse();
    } catch (err: unknown) {
      setDeleteError(toErrorMessage(err, '删除失败'));
    } finally {
      setDeletingUri(null);
    }
  };

  return (
    <div className="space-y-4">
      {/* Breadcrumb */}
      {browsePath.length > 0 && (
        <div className="flex items-center gap-1 text-xs text-[var(--color-text-secondary)]">
          <button onClick={() => setBrowsePath([])} className="hover:text-blue-600">
            知识库
          </button>
          {browsePath.map((seg, i) => (
            <span key={i} className="flex items-center gap-1">
              <ChevronRight size={12} />
              <button
                onClick={() => setBrowsePath(browsePath.slice(0, i + 1))}
                className="hover:text-blue-600"
              >
                {seg}
              </button>
            </span>
          ))}
        </div>
      )}
      {/* 查看层级选择 */}
      <div className="flex items-center gap-2 mb-2">
        <label className="text-xs text-[var(--color-text-tertiary)]">查看层级:</label>
        {(['abstract', 'overview', 'detail'] as const).map((lvl) => (
          <button
            key={lvl}
            onClick={() => setBrowseLevel(lvl)}
            className={`px-2 py-0.5 text-xs rounded border ${browseLevel === lvl ? 'border-blue-500 bg-blue-50 text-blue-700 dark:bg-blue-900/30 dark:text-blue-300' : 'border-[var(--color-border)] text-[var(--color-text-secondary)]'}`}
          >
            {lvl === 'abstract' ? 'L0 摘要' : lvl === 'overview' ? 'L1 概览' : 'L2 详情'}
          </button>
        ))}
        <button
          onClick={reloadBrowse}
          disabled={browseLoading}
          className="ml-auto px-2 py-0.5 text-xs rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]"
        >
          {browseLoading ? '刷新中...' : '刷新'}
        </button>
      </div>
      {browseError && (
        <div
          role="alert"
          className="flex items-center gap-2 p-3 rounded-lg bg-red-50 dark:bg-red-950 border border-red-200 dark:border-red-800 text-red-700 dark:text-red-300 text-sm"
        >
          <AlertCircle size={16} />
          <span>{browseError}</span>
        </div>
      )}
      {deleteError && (
        <div
          role="alert"
          className="flex items-center gap-2 p-3 rounded-lg bg-red-50 dark:bg-red-950 border border-red-200 dark:border-red-800 text-red-700 dark:text-red-300 text-sm"
        >
          <AlertCircle size={16} />
          <span>{deleteError}</span>
        </div>
      )}
      {browseLoading ? (
        <div className="flex items-center justify-center py-10">
          <Loader2 size={20} className="animate-spin text-[var(--color-text-tertiary)]" />
        </div>
      ) : (
        <div className="space-y-1">
          {browseEntries.map((entry) => (
            <div
              key={entry.uri}
              className={`flex items-center justify-between gap-2 px-3 py-2 rounded-md transition-colors ${selectedBrowseEntry?.uri === entry.uri ? 'bg-blue-50 dark:bg-blue-900/20 border border-blue-200 dark:border-blue-800' : 'border border-transparent hover:bg-[var(--color-bg-hover)]'}`}
            >
              <button
                type="button"
                onClick={() => handleBrowseView(entry)}
                className="flex items-center gap-2 min-w-0 flex-1 text-left cursor-pointer"
              >
                {entry.is_directory &&
                !(entry.has_abstract || entry.has_overview || entry.has_detail) ? (
                  <Folder size={16} className="shrink-0 text-yellow-500" />
                ) : (
                  <File size={16} className="shrink-0 text-[var(--color-text-tertiary)]" />
                )}
                <span className="text-sm text-[var(--color-text-primary)] truncate">
                  {entry.name}
                </span>
              </button>
              <button
                type="button"
                onClick={() => handleDeleteClick(entry)}
                disabled={deletingUri !== null}
                aria-label={`删除 ${entry.name}`}
                title={
                  entry.is_directory
                    ? `删除 ${entry.name}（将同时删除其全部子条目）`
                    : `删除 ${entry.name}`
                }
                className="p-1.5 rounded hover:bg-red-100 dark:hover:bg-red-900/30 text-[var(--color-text-tertiary)] hover:text-red-500 disabled:opacity-50 shrink-0"
              >
                {deletingUri === entry.uri ? (
                  <Loader2 size={14} className="animate-spin" />
                ) : (
                  <Trash2 size={14} />
                )}
              </button>
            </div>
          ))}
          {browseEntries.length === 0 && !browseLoading && !browseError && (
            <p className="text-sm text-[var(--color-text-tertiary)] py-8 text-center">
              知识库暂无条目
            </p>
          )}
        </div>
      )}

      {selectedBrowseEntry && (
        <div className="border-t border-[var(--color-border)] pt-4">
          <div className="flex items-center justify-between mb-3">
            <h4 className="text-sm font-medium text-[var(--color-text-primary)] truncate max-w-[70%]">
              {selectedBrowseEntry.name}
            </h4>
          </div>
          {browseContentLoading ? (
            <div className="flex items-center justify-center py-10">
              <Loader2 size={20} className="animate-spin text-[var(--color-text-tertiary)]" />
            </div>
          ) : (
            <pre className="max-h-[300px] overflow-y-auto p-4 rounded-lg bg-[var(--color-bg-secondary)] text-sm text-[var(--color-text-secondary)] whitespace-pre-wrap font-mono leading-relaxed border border-[var(--color-border)]">
              {browseContent || '(空内容)'}
            </pre>
          )}
        </div>
      )}

      {/* 删除确认（统一 ConfirmDialog 原语；5 秒未操作自动取消） */}
      <ConfirmDialog
        open={confirmTarget !== null}
        title="删除知识条目"
        message={
          confirmTarget
            ? `${confirmTarget.name} 将被永久删除${confirmTarget.isDirectory ? '（含全部子条目）' : ''}，5 秒后自动取消。`
            : ''
        }
        danger
        confirmLabel="删除"
        busy={deletingUri !== null}
        onConfirm={() => {
          if (confirmTarget) {
            void handleConfirmDelete(confirmTarget);
          }
        }}
        onCancel={() => setConfirmTarget(null)}
      />
    </div>
  );
}
