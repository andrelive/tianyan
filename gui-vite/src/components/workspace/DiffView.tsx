import { useEffect, useRef } from 'react';
import { EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { unifiedMergeView } from '@codemirror/merge';
import type { LanguageSupport } from '@codemirror/language';
import { AlertCircle, Loader2, Save, X } from 'lucide-react';
import Modal from '@/components/ui/Modal';

interface DiffViewProps {
  /** 磁盘上的原始内容（构造快照；变化时必须重建视图）。 */
  original: string;
  /** 当前（修改后）内容。 */
  modified: string;
  /** 语法高亮扩展（可选）。 */
  language?: LanguageSupport;
}

/**
 * 只读 unified diff 视图：@codemirror/merge 的 unifiedMergeView，
 * 当前内容在上、被删除的原文以折叠块形式插入其中，改动高亮。
 *
 * Phase 2 用途：保存前的 diff 确认弹窗（original = 磁盘内容，modified = 编辑结果）。
 * original 是构造快照，变化时必须整体重建 EditorView。
 */
export default function DiffView({ original, modified, language }: DiffViewProps) {
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!containerRef.current) return;
    const extensions = [
      EditorState.readOnly.of(true),
      EditorView.editable.of(false),
      ...(language ? [language] : []),
      unifiedMergeView({
        original,
        mergeControls: false,
        gutter: true,
        highlightChanges: true,
        collapseUnchanged: { margin: 3, minSize: 6 },
      }),
      EditorView.theme({
        '&': { height: '100%', fontSize: '13px' },
        '.cm-scroller': { overflow: 'auto' },
        '&.cm-focused': { outline: 'none' },
      }),
    ];
    const state = EditorState.create({ doc: modified, extensions });
    const view = new EditorView({ state, parent: containerRef.current });
    return () => {
      view.destroy();
    };
  }, [original, modified, language]);

  return <div ref={containerRef} className="h-full min-h-0 overflow-hidden" />;
}

interface SaveConfirmDialogProps {
  /** 正在保存的文件相对路径（标题展示 + 语言推断）。 */
  path: string;
  /** 磁盘上的原始内容（剥离 hashline 后）。 */
  original: string;
  /** 编辑后的当前内容。 */
  modified: string;
  /** 语法高亮扩展（可选）。 */
  language?: LanguageSupport | null;
  /** 提交中（禁用按钮，阻止关闭）。 */
  saving: boolean;
  /** 提交失败信息（弹窗内展示）。 */
  error: string | null;
  onCancel: () => void;
  onConfirm: () => void;
}

/**
 * 保存前 diff 确认弹窗：原生 fixed overlay（不引 UI 库），
 * 内含只读 DiffView + 确认保存 / 取消按钮，失败时在弹窗内展示错误。
 */
export function SaveConfirmDialog({
  path,
  original,
  modified,
  language,
  saving,
  error,
  onCancel,
  onConfirm,
}: SaveConfirmDialogProps) {
  return (
    <Modal
      onClose={onCancel}
      closeDisabled={saving}
      ariaLabel="保存确认"
      panelClassName="flex flex-col w-full max-w-3xl h-[70vh]"
    >
        <div className="flex items-center justify-between px-4 py-2.5 border-b border-[var(--color-border)] shrink-0">
          <h3 className="text-sm font-medium text-[var(--color-text-primary)] truncate">
            保存修改 · <span className="font-mono">{path}</span>
          </h3>
          <button
            type="button"
            onClick={onCancel}
            disabled={saving}
            aria-label="关闭保存确认"
            className="p-1 rounded hover:bg-[var(--color-bg-hover)] text-[var(--color-text-secondary)] disabled:opacity-50"
          >
            <X size={16} />
          </button>
        </div>

        <div className="flex-1 min-h-0 border-b border-[var(--color-border)]">
          <DiffView original={original} modified={modified} language={language ?? undefined} />
        </div>

        <div className="shrink-0 px-4 py-3 space-y-2">
          {error && (
            <div
              role="alert"
              className="flex items-center gap-2 p-2.5 rounded-lg bg-red-50 dark:bg-red-950 border border-red-200 dark:border-red-800 text-red-700 dark:text-red-300 text-xs"
            >
              <AlertCircle size={14} className="shrink-0" />
              <span>{error}</span>
            </div>
          )}
          <div className="flex items-center justify-end gap-2">
            <button
              type="button"
              onClick={onCancel}
              disabled={saving}
              className="px-3 py-1.5 text-xs font-medium rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50"
            >
              取消
            </button>
            <button
              type="button"
              onClick={onConfirm}
              disabled={saving}
              className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded bg-blue-600 text-white hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {saving ? <Loader2 size={12} className="animate-spin" /> : <Save size={12} />}
              {saving ? '保存中…' : '确认保存'}
            </button>
          </div>
        </div>
    </Modal>
  );
}
