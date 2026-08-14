import { useEffect, useState } from 'react';
import {
  AlertCircle,
  FolderTree,
  FolderOpen,
  GitCompareArrows,
  Loader2,
  PanelLeft,
  PanelLeftClose,
  PanelRightClose,
} from 'lucide-react';
import { createTwoFilesPatch } from 'diff';
import {
  ApiError,
  fetchWorkspaceApplyPatch,
  fetchWorkspaceDiff,
  fetchWorkspaceDiffList,
  updateSessionWorkspace,
} from '@/lib/api-client';
import type { WorkspaceDiffListResponse, WorkspaceDiffResponse } from '@/lib/types';
import { useAppStore } from '@/lib/store';
import WorkspaceTree from './WorkspaceTree';
import WorkspacePicker from './WorkspacePicker';
import FileViewer from './FileViewer';
import type { FileViewerDraft } from './FileViewer';
import { languageForPath } from './fileLanguage';
import { SaveConfirmDialog } from './DiffView';

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
function DiffPanel({ filePath, onClose }: DiffPanelProps) {
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

  const loadFileDiff = async (path: string, session: string) => {
    const indexNum = Number(indexText);
    setLoading(true);
    setError(null);
    try {
      const res = await fetchWorkspaceDiff(
        path,
        session,
        Number.isFinite(indexNum) ? indexNum : undefined,
      );
      setDiff(res);
    } catch (err: unknown) {
      setDiff(null);
      setError(err instanceof Error ? err.message : '加载 diff 失败');
    } finally {
      setLoading(false);
    }
  };

  const handleLoad = async () => {
    if (!filePath) return;
    const trimmed = sessionId.trim();
    if (!trimmed) {
      setSessionIdError('请填写会话 ID');
      return;
    }
    setSessionIdError(null);
    await loadFileDiff(filePath, trimmed);
  };

  /** 切到整体差异：无 path 调用后端，返回工作区全部变更文件。 */
  const handleWorkspaceMode = async () => {
    const trimmed = sessionId.trim();
    if (!trimmed) {
      setSessionIdError('请填写会话 ID');
      return;
    }
    setSessionIdError(null);
    setMode('workspace');
    const indexNum = Number(indexText);
    setLoading(true);
    setError(null);
    setDiff(null);
    try {
      const res = await fetchWorkspaceDiffList(
        trimmed,
        Number.isFinite(indexNum) ? indexNum : undefined,
      );
      setDiffList(res);
    } catch (err: unknown) {
      setDiffList(null);
      setError(err instanceof Error ? err.message : '加载整体 diff 失败');
    } finally {
      setLoading(false);
    }
  };

  /** 点击整体差异列表中的文件行 → 切回单文件模式并加载该文件 diff。 */
  const selectFileFromList = async (item: WorkspaceDiffResponse) => {
    if (!item.path) return;
    const trimmed = sessionId.trim();
    if (!trimmed) {
      setSessionIdError('请填写会话 ID');
      return;
    }
    setSessionIdError(null);
    setMode('file');
    await loadFileDiff(item.path, trimmed);
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
          <span className="block text-xs text-[var(--color-text-tertiary)] mb-1">会话 ID</span>
          <input
            type="text"
            value={sessionId}
            onChange={(e) => {
              setSessionId(e.target.value);
              if (sessionIdError) setSessionIdError(null);
            }}
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

/** 工作区页面：左侧文件树 + 中间文件查看器 + 右侧 diff 面板（默认隐藏）。 */
export default function WorkspacePanel() {
  const currentSessionId = useAppStore((s) => s.currentSessionId);
  const sessions = useAppStore((s) => s.sessions);
  const newSessionWorkspace = useAppStore((s) => s.newSessionWorkspace);
  const setNewSessionWorkspace = useAppStore((s) => s.setNewSessionWorkspace);
  const showToast = useAppStore((s) => s.showToast);
  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  const [treeCollapsed, setTreeCollapsed] = useState(false);
  const [diffOpen, setDiffOpen] = useState(false);
  /** 工作区目录选择器（选择工作目录）。 */
  const [pickerOpen, setPickerOpen] = useState(false);
  const [pickerSaving, setPickerSaving] = useState(false);
  const [workspaceError, setWorkspaceError] = useState<string | null>(null);

  // ---- 保存前 diff 确认（编辑模式 → 保存 → 弹窗 → 确认 → apply-patch） ----
  const [saveDraft, setSaveDraft] = useState<FileViewerDraft | null>(null);
  const [saveSaving, setSaveSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  /** 保存成功后自增，传给 FileViewer 触发重新读取磁盘内容。 */
  const [reloadKey, setReloadKey] = useState(0);

  const handleSelectFile = (path: string) => {
    setSelectedFile(path);
  };

  // 当前会话绑定的工作目录（工作区 = 会话的父级分组）
  const currentSession = sessions.find((s) => s.id === currentSessionId) ?? null;
  const activeWorkspaceLabel = currentSession?.working_directory
    ? `目录：${currentSession.working_directory}`
    : currentSessionId
      ? '目录：全局默认'
      : newSessionWorkspace
        ? `新会话目录：${newSessionWorkspace}`
        : '未选择会话（浏览默认目录）';

  /**
   * 选择工作目录（工作区 = 会话的父级分组）：
   * - 有活动会话 → 绑定到当前会话（PUT /sessions/:id/workspace）；
   * - 无活动会话 → 作为下一个新对话的待绑定工作区。
   */
  const handleSelectWorkingDir = async (path: string) => {
    setPickerSaving(true);
    setWorkspaceError(null);
    try {
      if (currentSessionId) {
        const updated = await updateSessionWorkspace(currentSessionId, path);
        // 同步本地会话列表中的工作区归属（侧边栏分组即时更新）
        const current = useAppStore.getState().sessions;
        useAppStore
          .getState()
          .setSessions(
            current.map((s) =>
              s.id === currentSessionId
                ? { ...s, working_directory: updated.working_directory ?? null }
                : s,
            ),
          );
      } else {
        setNewSessionWorkspace(path || null);
        if (path) {
          showToast(`已设置：新对话将绑定工作区 ${path}`, 'success');
        }
      }
      setPickerOpen(false);
      setReloadKey((k) => k + 1);
    } catch (err: unknown) {
      setWorkspaceError(err instanceof Error ? err.message : '保存工作目录失败');
    } finally {
      setPickerSaving(false);
    }
  };

  const handleSaveRequest = (draft: FileViewerDraft) => {
    setSaveDraft(draft);
    setSaveError(null);
    setSaveSaving(false);
  };

  const handleCloseSaveDialog = () => {
    if (saveSaving) return;
    setSaveDraft(null);
    setSaveError(null);
  };

  const handleConfirmSave = async () => {
    if (!saveDraft) return;
    setSaveSaving(true);
    setSaveError(null);
    try {
      // jsdiff 生成的 unified patch：`--- a/path` `+++ b/path` 头，直接提交给后端。
      const patch = createTwoFilesPatch(
        `a/${saveDraft.path}`,
        `b/${saveDraft.path}`,
        saveDraft.original,
        saveDraft.modified,
      );
      await fetchWorkspaceApplyPatch(patch, currentSessionId ?? undefined);
      setSaveDraft(null);
      setReloadKey((k) => k + 1);
    } catch (err: unknown) {
      // 409 = 补丁定位失败/内容不匹配（文件在编辑期间被改动）。
      const isConflict = err instanceof ApiError && err.code === '409';
      setSaveError(
        isConflict ? '文件已被修改，请重新加载' : err instanceof Error ? err.message : '保存失败',
      );
    } finally {
      setSaveSaving(false);
    }
  };

  return (
    <div className="flex flex-col h-full min-h-0">
      {/* Header */}
      <div className="flex items-center justify-between px-6 py-4 border-b border-[var(--color-border)] shrink-0">
        <div className="flex items-center gap-3 min-w-0">
          <h2 className="text-lg font-semibold text-[var(--color-text-primary)] shrink-0">文件</h2>
          <span
            className="text-xs text-[var(--color-text-tertiary)] truncate"
            title={activeWorkspaceLabel}
            aria-label={activeWorkspaceLabel}
          >
            {activeWorkspaceLabel}
          </span>
        </div>
        <div className="flex items-center gap-1">
          <button
            type="button"
            onClick={() => setPickerOpen(true)}
            aria-label="选择目录"
            className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded-md border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] transition-colors"
          >
            <FolderOpen size={14} />
            选择目录
          </button>

          <button
            type="button"
            onClick={() => setDiffOpen((prev) => !prev)}
            aria-label={diffOpen ? '收起 diff 面板' : '展开 diff 面板'}
            aria-pressed={diffOpen}
            className={`flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded-md border transition-colors ${
              diffOpen
                ? 'border-blue-500 bg-blue-50 text-blue-700 dark:bg-blue-900/30 dark:text-blue-300'
                : 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]'
            }`}
          >
            <GitCompareArrows size={14} />
            Diff
          </button>
        </div>
      </div>

      <div className="flex flex-1 min-h-0">
        {/* 左：文件树（可折叠） */}
        {treeCollapsed ? (
          <button
            type="button"
            onClick={() => setTreeCollapsed(false)}
            aria-label="展开文件树"
            title="展开文件树"
            className="w-9 shrink-0 flex items-start justify-center pt-3 border-r border-[var(--color-border)] bg-[var(--color-bg-secondary)] text-[var(--color-text-tertiary)] hover:text-[var(--color-text-secondary)]"
          >
            <PanelLeft size={16} />
          </button>
        ) : (
          <aside className="w-64 shrink-0 flex flex-col border-r border-[var(--color-border)] bg-[var(--color-bg-secondary)] min-h-0">
            <div className="flex items-center justify-between px-3 py-2 border-b border-[var(--color-border)]">
              <span className="flex items-center gap-1.5 text-xs font-medium text-[var(--color-text-secondary)]">
                <FolderTree size={13} />
                文件
              </span>
              <button
                type="button"
                onClick={() => setTreeCollapsed(true)}
                aria-label="收起文件树"
                className="p-1 rounded hover:bg-[var(--color-bg-hover)] text-[var(--color-text-tertiary)]"
              >
                <PanelLeftClose size={14} />
              </button>
            </div>
            <div className="flex-1 min-h-0">
              <WorkspaceTree
                onSelectFile={handleSelectFile}
                sessionId={currentSessionId ?? undefined}
              />
            </div>
          </aside>
        )}

        {/* 中：文件查看器 */}
        <main className="flex-1 min-w-0 flex flex-col bg-[var(--color-bg-primary)]">
          <FileViewer
            path={selectedFile}
            reloadKey={reloadKey}
            onSaveRequest={handleSaveRequest}
            sessionId={currentSessionId ?? undefined}
          />
        </main>

        {/* 右：diff 面板（默认隐藏） */}
        {diffOpen && <DiffPanel filePath={selectedFile} onClose={() => setDiffOpen(false)} />}
      </div>

      {/* 保存前 diff 确认弹窗 */}
      {saveDraft && (
        <SaveConfirmDialog
          path={saveDraft.path}
          original={saveDraft.original}
          modified={saveDraft.modified}
          language={languageForPath(saveDraft.path)}
          saving={saveSaving}
          error={saveError}
          onCancel={handleCloseSaveDialog}
          onConfirm={handleConfirmSave}
        />
      )}

      {/* 工作目录选择器 */}
      <WorkspacePicker
        open={pickerOpen}
        currentWorkingDir={currentSession?.working_directory ?? newSessionWorkspace ?? ''}
        onClose={() => setPickerOpen(false)}
        onSelect={handleSelectWorkingDir}
        saving={pickerSaving}
        clearLabel={currentSession?.working_directory ? '清除绑定' : '不绑定目录'}
      />
      {workspaceError && (
        <p className="px-6 py-2 text-xs text-[var(--color-error)]" role="alert">
          {workspaceError}
        </p>
      )}
    </div>
  );
}
