import { useState, useEffect, useRef, useCallback } from 'react';
import {
  FileText,
  Folder,
  ChevronRight,
  Loader2,
  AlertCircle,
  Edit3,
  Trash2,
  Save,
  X,
  Plus,
} from 'lucide-react';
import {
  fetchNamespaces,
  fetchVfsEntries,
  fetchVfsEntryContent,
  updateVfsEntry,
  deleteVfsEntry,
  createVfsEntry,
  type NamespaceInfo,
  type EntryItem,
} from '@/lib/api-client';
import { useAppStore } from '@/lib/store';

export default function VfsTab() {
  const showToast = useAppStore((s) => s.showToast);

  const [namespaces, setNamespaces] = useState<NamespaceInfo[]>([]);
  const [selectedNs, setSelectedNs] = useState('knowledge');
  const [currentPath, setCurrentPath] = useState<string[]>([]);
  const [entries, setEntries] = useState<EntryItem[]>([]);
  const [selectedEntry, setSelectedEntry] = useState<EntryItem | null>(null);
  const [entryContent, setEntryContent] = useState('');
  const [entryLevel, setEntryLevel] = useState('detail');
  const [isEditing, setIsEditing] = useState(false);
  const [editContent, setEditContent] = useState('');
  const [loading, setLoading] = useState(true);
  const [loadingEntries, setLoadingEntries] = useState(false);
  const [loadingContent, setLoadingContent] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const reqIdRef = useRef(0);

  // Create state
  const [showCreate, setShowCreate] = useState(false);
  const [newName, setNewName] = useState('');
  const [newIsDir, setNewIsDir] = useState(false);
  const [newContent, setNewContent] = useState('');
  const [creating, setCreating] = useState(false);

  // Navigate to a subdirectory
  const navigateInto = useCallback(
    (entry: EntryItem) => {
      if (entry.is_directory) {
        setCurrentPath((prev) => [...prev, entry.name]);
        setSelectedEntry(null);
      } else {
        handleViewEntry(entry);
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [entryLevel],
  );

  // Navigate up to a specific breadcrumb level
  const navigateTo = useCallback((index: number) => {
    setCurrentPath((prev) => prev.slice(0, index));
    setSelectedEntry(null);
  }, []);

  // Change namespace and reset path
  const switchNamespace = useCallback((ns: string) => {
    setSelectedNs(ns);
    setCurrentPath([]);
    setSelectedEntry(null);
    setIsEditing(false);
  }, []);

  useEffect(() => {
    fetchNamespaces()
      .then((res) => {
        setNamespaces(res.namespaces);
        setLoading(false);
      })
      .catch((err: Error) => {
        setError(err.message);
        setLoading(false);
      });
  }, []);

  useEffect(() => {
    setEntries([]);
    setSelectedEntry(null);
    setError(null);
    setLoadingEntries(true);
    const id = ++reqIdRef.current;
    const pathStr = currentPath.join('/') || undefined;
    fetchVfsEntries(selectedNs, pathStr)
      .then((res) => {
        if (id === reqIdRef.current) {
          setEntries(res.entries);
          setLoadingEntries(false);
        }
      })
      .catch((err: Error) => {
        if (id === reqIdRef.current) {
          setError(err.message);
          setLoadingEntries(false);
        }
      });
  }, [selectedNs, currentPath]);

  const handleViewEntry = async (entry: EntryItem) => {
    if (entry.is_directory) return;
    setSelectedEntry(entry);
    setIsEditing(false);
    setLoadingContent(true);
    try {
      const res = await fetchVfsEntryContent(entry.uri, entryLevel);
      setEntryContent(res.content);
    } catch (err: unknown) {
      setEntryContent(`加载失败: ${err instanceof Error ? err.message : '未知错误'}`);
    } finally {
      setLoadingContent(false);
    }
  };

  const handleStartEdit = () => {
    setEditContent(entryContent);
    setIsEditing(true);
  };

  const handleSaveEdit = async () => {
    if (!selectedEntry) return;
    try {
      await updateVfsEntry(selectedEntry.uri, entryLevel, editContent);
      setEntryContent(editContent);
      setIsEditing(false);
      showToast('保存成功', 'success');
    } catch (err: unknown) {
      showToast(`保存失败: ${err instanceof Error ? err.message : '未知错误'}`, 'error');
    }
  };

  const handleDelete = async (entry: EntryItem) => {
    if (!window.confirm(`确定要删除 "${entry.name}" 吗？此操作不可撤销。`)) return;
    try {
      await deleteVfsEntry(entry.uri);
      setEntries((prev) => prev.filter((e) => e.uri !== entry.uri));
      if (selectedEntry?.uri === entry.uri) {
        setSelectedEntry(null);
        setIsEditing(false);
      }
      showToast('已删除', 'success');
    } catch (err: unknown) {
      showToast(`删除失败: ${err instanceof Error ? err.message : '未知错误'}`, 'error');
    }
  };

  const handleCreate = async () => {
    if (!newName.trim()) return;
    setCreating(true);
    const baseUri = `tianyan://${selectedNs}${
      currentPath.length ? '/' + currentPath.join('/') : ''
    }`;
    const uri = `${baseUri}/${newName.trim()}`;
    try {
      await createVfsEntry({
        uri,
        is_directory: newIsDir,
        detail_content: newContent || undefined,
      });
      showToast(`已创建 ${newIsDir ? '目录' : '文件'}: ${newName}`, 'success');
      setShowCreate(false);
      setNewName('');
      setNewContent('');
      setNewIsDir(false);
      // Refresh list
      const pathStr = currentPath.join('/') || undefined;
      const res = await fetchVfsEntries(selectedNs, pathStr);
      setEntries(res.entries);
    } catch (err: unknown) {
      showToast(`创建失败: ${err instanceof Error ? err.message : '未知错误'}`, 'error');
    } finally {
      setCreating(false);
    }
  };

  if (loading) {
    return (
      <div className="flex items-center justify-center py-20">
        <Loader2 size={24} className="animate-spin text-[var(--color-text-tertiary)]" />
      </div>
    );
  }

  return (
    <div>
      <div className="flex items-center justify-between mb-4">
        <h3 className="text-base font-semibold text-[var(--color-text-primary)]">VFS 浏览器</h3>
      </div>

      <p className="text-sm text-[var(--color-text-secondary)] mb-4">
        浏览虚拟文件系统中的所有命名空间和条目内容。
      </p>

      {/* Namespace selector */}
      <div className="flex gap-2 mb-4">
        {namespaces.map((ns) => (
          <button
            key={ns.name}
            onClick={() => switchNamespace(ns.name)}
            className={`px-3 py-1.5 text-xs font-medium rounded-md transition-colors ${
              selectedNs === ns.name
                ? 'bg-blue-600 text-white'
                : 'bg-[var(--color-bg-secondary)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]'
            }`}
          >
            {ns.display}
          </button>
        ))}
      </div>

      {/* Breadcrumb */}
      <div className="flex items-center gap-1 mb-3 text-xs text-[var(--color-text-secondary)]">
        <button onClick={() => navigateTo(0)} className="hover:text-blue-600 transition-colors">
          {namespaces.find((n) => n.name === selectedNs)?.display || selectedNs}
        </button>
        {currentPath.map((seg, i) => (
          <span key={i} className="flex items-center gap-1">
            <ChevronRight size={12} />
            <button
              onClick={() => navigateTo(i + 1)}
              className="hover:text-blue-600 transition-colors"
            >
              {seg}
            </button>
          </span>
        ))}
      </div>

      {/* Level selector */}
      <div className="flex items-center gap-2 mb-4">
        <label className="text-xs text-[var(--color-text-tertiary)]">查看层级:</label>
        {['abstract', 'overview', 'detail'].map((lvl) => (
          <button
            key={lvl}
            onClick={() => setEntryLevel(lvl)}
            className={`px-2 py-0.5 text-xs rounded border transition-colors ${
              entryLevel === lvl
                ? 'border-blue-500 bg-blue-50 text-blue-700 dark:bg-blue-900/30 dark:text-blue-300'
                : 'border-[var(--color-border)] text-[var(--color-text-secondary)]'
            }`}
          >
            {lvl === 'abstract' ? 'L0 摘要' : lvl === 'overview' ? 'L1 概览' : 'L2 详情'}
          </button>
        ))}
      </div>

      {error && (
        <div
          role="alert"
          className="mb-4 flex items-center gap-2 p-3 rounded-lg bg-red-50 dark:bg-red-950 border border-red-200 dark:border-red-800 text-red-700 dark:text-red-300 text-sm"
        >
          <AlertCircle size={16} />
          <span>{error}</span>
        </div>
      )}

      {/* Entry list */}
      {loadingEntries ? (
        <div className="flex items-center justify-center py-10">
          <Loader2 size={20} className="animate-spin text-[var(--color-text-tertiary)]" />
        </div>
      ) : (
        <div className="space-y-1 mb-4">
          {entries.map((entry) => (
            <div
              key={entry.uri}
              onClick={() => navigateInto(entry)}
              className={`flex items-center justify-between px-3 py-2 rounded-md cursor-pointer transition-colors ${
                selectedEntry?.uri === entry.uri
                  ? 'bg-blue-50 dark:bg-blue-900/20 border border-blue-200 dark:border-blue-800'
                  : 'border border-transparent hover:bg-[var(--color-bg-hover)]'
              }`}
            >
              <div className="flex items-center gap-2 min-w-0">
                {entry.is_directory ? (
                  <Folder size={16} className="shrink-0 text-yellow-500" />
                ) : (
                  <FileText size={16} className="shrink-0 text-[var(--color-text-tertiary)]" />
                )}
                <span className="text-sm text-[var(--color-text-primary)] truncate">
                  {entry.name}
                </span>
              </div>
              <div className="flex items-center gap-2 shrink-0">
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    handleDelete(entry);
                  }}
                  className="p-1 rounded hover:bg-red-100 dark:hover:bg-red-900/30 text-[var(--color-text-tertiary)] hover:text-red-500"
                  title="删除"
                >
                  <Trash2 size={14} />
                </button>
              </div>
            </div>
          ))}
          {entries.length === 0 && !loadingEntries && !error && (
            <p className="text-sm text-[var(--color-text-tertiary)] py-8 text-center">
              该目录下暂无条目
            </p>
          )}
        </div>
      )}

      {/* Create button & form */}
      {!showCreate ? (
        <button
          onClick={() => setShowCreate(true)}
          className="flex items-center gap-1 px-3 py-1.5 text-xs font-medium rounded-md border border-dashed border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] hover:border-blue-400 hover:text-blue-600 transition-colors"
        >
          <Plus size={14} /> 新建条目
        </button>
      ) : (
        <div className="mb-6 p-4 rounded-lg border border-blue-200 dark:border-blue-800 bg-blue-50/50 dark:bg-blue-900/10 space-y-3">
          <div className="flex items-center gap-3">
            <input
              type="text"
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
              placeholder="条目名称"
              autoFocus
              className="flex-1 px-2.5 py-1.5 text-sm rounded border border-[var(--color-border)] bg-[var(--color-bg-primary)] focus:outline-none focus:ring-2 focus:ring-blue-500/30"
            />
            <label className="flex items-center gap-1.5 text-xs text-[var(--color-text-secondary)]">
              <input
                type="checkbox"
                checked={newIsDir}
                onChange={(e) => setNewIsDir(e.target.checked)}
                className="rounded"
              />
              目录
            </label>
          </div>
          {!newIsDir && (
            <textarea
              value={newContent}
              onChange={(e) => setNewContent(e.target.value)}
              placeholder="内容（可选）"
              rows={3}
              className="w-full px-2.5 py-1.5 text-sm rounded border border-[var(--color-border)] bg-[var(--color-bg-primary)] font-mono focus:outline-none focus:ring-2 focus:ring-blue-500/30"
            />
          )}
          <div className="flex items-center gap-2">
            <button
              onClick={handleCreate}
              disabled={creating || !newName.trim()}
              className="flex items-center gap-1 px-3 py-1 text-xs font-medium rounded bg-blue-600 text-white hover:bg-blue-700 disabled:opacity-50 transition-colors"
            >
              {creating ? <Loader2 size={12} className="animate-spin" /> : <Save size={12} />}
              {creating ? '创建中...' : '创建'}
            </button>
            <button
              onClick={() => { setShowCreate(false); setNewName(''); setNewContent(''); }}
              className="flex items-center gap-1 px-3 py-1 text-xs rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]"
            >
              <X size={12} /> 取消
            </button>
          </div>
        </div>
      )}

      {/* Content viewer / editor */}
      {selectedEntry && (
        <div className="border-t border-[var(--color-border)] pt-4">
          <div className="flex items-center justify-between mb-3">
            <h4 className="text-sm font-medium text-[var(--color-text-primary)] truncate max-w-[70%]">
              {selectedEntry.name}
            </h4>
            <div className="flex items-center gap-2">
              {!isEditing ? (
                <button
                  onClick={handleStartEdit}
                  className="flex items-center gap-1 px-2 py-1 text-xs rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]"
                >
                  <Edit3 size={12} />
                  编辑
                </button>
              ) : (
                <>
                  <button
                    onClick={handleSaveEdit}
                    className="flex items-center gap-1 px-2 py-1 text-xs rounded bg-green-600 text-white hover:bg-green-700"
                  >
                    <Save size={12} />
                    保存
                  </button>
                  <button
                    onClick={() => setIsEditing(false)}
                    className="flex items-center gap-1 px-2 py-1 text-xs rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]"
                  >
                    <X size={12} />
                    取消
                  </button>
                </>
              )}
            </div>
          </div>

          {loadingContent ? (
            <div className="flex items-center justify-center py-10">
              <Loader2 size={20} className="animate-spin text-[var(--color-text-tertiary)]" />
            </div>
          ) : isEditing ? (
            <textarea
              value={editContent}
              onChange={(e) => setEditContent(e.target.value)}
              className="w-full min-h-[300px] px-3 py-2 rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-sm text-[var(--color-text-primary)] font-mono focus:outline-none focus:ring-2 focus:ring-blue-500/30"
            />
          ) : (
            <pre className="max-h-[400px] overflow-y-auto p-4 rounded-lg bg-[var(--color-bg-secondary)] text-sm text-[var(--color-text-secondary)] whitespace-pre-wrap font-mono leading-relaxed border border-[var(--color-border)]">
              {entryContent || '(空内容)'}
            </pre>
          )}
        </div>
      )}
    </div>
  );
}
