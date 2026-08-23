import { useState } from 'react';
import { FolderTree, FolderOpen, GitCompareArrows, PanelLeft, PanelLeftClose } from 'lucide-react';
import { createTwoFilesPatch } from 'diff';
import { isConflict, fetchWorkspaceApplyPatch, updateSessionWorkspace } from '@/lib/api-client';
import { useAppStore } from '@/lib/store';
import WorkspaceTree from './WorkspaceTree';
import WorkspacePicker from './WorkspacePicker';
import FileViewer from './FileViewer';
import type { FileViewerDraft } from './FileViewer';
import { languageForPath } from './fileLanguage';
import { SaveConfirmDialog } from './DiffView';
import WorkspaceDiffPanel from './WorkspaceDiffPanel';

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
      const patchConflict = isConflict(err);
      setSaveError(
        patchConflict
          ? '文件已被修改，请重新加载'
          : err instanceof Error
            ? err.message
            : '保存失败',
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
        {diffOpen && (
          <WorkspaceDiffPanel filePath={selectedFile} onClose={() => setDiffOpen(false)} />
        )}
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
