import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { EditorState } from '@codemirror/state';
import {
  EditorView,
  lineNumbers,
  highlightActiveLineGutter,
  highlightSpecialChars,
  drawSelection,
  dropCursor,
  rectangularSelection,
  crosshairCursor,
  highlightActiveLine,
  keymap,
} from '@codemirror/view';
import {
  syntaxHighlighting,
  defaultHighlightStyle,
  foldGutter,
  bracketMatching,
  indentOnInput,
} from '@codemirror/language';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import {
  AlertCircle,
  Binary,
  FileCode2,
  Hash,
  Loader2,
  Pencil,
  RotateCcw,
  Save,
} from 'lucide-react';
import { fetchWorkspaceRead } from '@/lib/api-client';
import { computeNextOffset, stripHashline } from './fileViewerUtils';
import { toErrorMessage } from '@/lib/errors';
import type { WorkspaceReadResponse } from '@/lib/types';
import { languageForPath } from './fileLanguage';

const PAGE_LIMIT = 2000;

/** 保存请求载荷：path 为相对工作区路径，original/modified 均为剥离 hashline 的内容。 */
export interface FileViewerDraft {
  path: string;
  original: string;
  modified: string;
}

interface FileViewerProps {
  /** 当前选中的文件相对路径；null 时显示占位提示。 */
  path: string | null;
  /** 保存成功后父组件自增，触发重新读取磁盘内容并退出编辑模式。 */
  reloadKey?: number;
  /** 用户点击"保存"时携带磁盘原文与编辑结果通知父组件（打开 diff 确认弹窗）。 */
  onSaveRequest?: (draft: FileViewerDraft) => void;
  /** 会话 ID（解析会话绑定的工作目录；缺省 = 全局配置）。 */
  sessionId?: string;
}

/** 只读/可编辑 CodeMirror 6 文件查看器：hashline 剥离、二进制提示、分页加载更多、编辑模式。 */
export default function FileViewer({
  path,
  reloadKey = 0,
  onSaveRequest,
  sessionId,
}: FileViewerProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [response, setResponse] = useState<WorkspaceReadResponse | null>(null);
  const [rawLines, setRawLines] = useState<string[]>([]);
  const [nextOffset, setNextOffset] = useState(0);
  const [showAnchors, setShowAnchors] = useState(false);
  const [loadMoreLoading, setLoadMoreLoading] = useState(false);

  // ---- 编辑模式状态 ----
  const [editing, setEditing] = useState(false);
  const [editLoading, setEditLoading] = useState(false);
  const [dirty, setDirty] = useState(false);
  /** 磁盘原文（剥离 hashline），进入编辑时快照；保存时作为 diff 的 original。 */
  const [originalText, setOriginalText] = useState('');
  /** 编辑后的当前内容（由 CM updateListener 提升）。 */
  const [draftText, setDraftText] = useState('');

  const loadFile = useCallback(
    async (filePath: string) => {
      setLoading(true);
      setError(null);
      setResponse(null);
      setRawLines([]);
      setNextOffset(0);
      setShowAnchors(false);
      setEditing(false);
      setEditLoading(false);
      setDirty(false);
      setOriginalText('');
      setDraftText('');
      try {
        const res = await fetchWorkspaceRead(filePath, undefined, PAGE_LIMIT, sessionId);
        setResponse(res);
        if (res.content !== undefined) {
          const lines = res.content.split('\n');
          setRawLines(lines);
          setNextOffset(computeNextOffset(res));
        }
      } catch (err: unknown) {
        setError(toErrorMessage(err, '加载失败'));
      } finally {
        setLoading(false);
      }
    },
    [sessionId],
  );

  useEffect(() => {
    if (path) {
      loadFile(path);
    } else {
      setResponse(null);
      setRawLines([]);
      setNextOffset(0);
      setEditing(false);
      setEditLoading(false);
      setDirty(false);
      setOriginalText('');
      setDraftText('');
    }
  }, [path, reloadKey, loadFile]);

  const handleLoadMore = async () => {
    if (!path) return;
    setLoadMoreLoading(true);
    try {
      const res = await fetchWorkspaceRead(path, nextOffset, PAGE_LIMIT, sessionId);
      setResponse((prev) => ({ ...prev, ...res }));
      if (res.content !== undefined) {
        const lines = res.content.split('\n');
        setRawLines((prev) => [...prev, ...lines]);
        setNextOffset(computeNextOffset(res));
      }
    } catch (err: unknown) {
      setError(toErrorMessage(err, '加载失败'));
    } finally {
      setLoadMoreLoading(false);
    }
  };

  /**
   * 进入编辑模式：若内容被截断，先循环拉完剩余页（编辑需要完整内容），
   * 然后以剥离 hashline 的全量文本创建可编辑实例。
   */
  const handleEnterEdit = async () => {
    if (!path || !response || response.binary || editLoading) return;
    setEditLoading(true);
    try {
      let lines = rawLines;
      let offset = nextOffset;
      let truncated = response.truncated ?? false;
      while (truncated) {
        const res = await fetchWorkspaceRead(path, offset, PAGE_LIMIT, sessionId);
        setResponse((prev) => ({ ...prev, ...res }));
        if (res.content !== undefined) {
          lines = [...lines, ...res.content.split('\n')];
          offset = computeNextOffset(res);
          truncated = res.truncated ?? false;
        } else {
          truncated = false;
        }
      }
      const fullText = lines.map(stripHashline).join('\n');
      setOriginalText(fullText);
      setDraftText(fullText);
      setDirty(false);
      setEditing(true);
    } catch (err: unknown) {
      setError(toErrorMessage(err, '加载完整内容失败'));
    } finally {
      setEditLoading(false);
    }
  };

  /** 放弃：恢复到磁盘内容并退出编辑模式（dirty 清除，视图重建为只读）。 */
  const handleDiscard = () => {
    setEditing(false);
    setEditLoading(false);
    setDirty(false);
    setDraftText('');
    setOriginalText('');
  };

  const handleSave = () => {
    if (!path || !editing || !dirty) return;
    onSaveRequest?.({ path, original: originalText, modified: draftText });
  };

  const langExt = useMemo(() => (path ? languageForPath(path) : null), [path]);

  // CodeMirror 实例：内容/语言/锚点开关/编辑模式变化时重建。
  // 编辑模式下 doc 权威在视图内（draftText 不参与依赖），输入不触发重建。
  useEffect(() => {
    if (!containerRef.current || response?.binary) return;
    const doc = editing
      ? originalText
      : showAnchors
        ? rawLines.join('\n')
        : rawLines.map(stripHashline).join('\n');
    const extensions = [
      lineNumbers(),
      highlightActiveLineGutter(),
      highlightSpecialChars(),
      drawSelection(),
      dropCursor(),
      rectangularSelection(),
      crosshairCursor(),
      highlightActiveLine(),
      foldGutter(),
      bracketMatching(),
      indentOnInput(),
      syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
    ];
    if (editing) {
      extensions.push(
        history(),
        keymap.of(defaultKeymap),
        keymap.of(historyKeymap),
        keymap.of([indentWithTab]),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) {
            const text = update.state.doc.toString();
            setDraftText(text);
            setDirty(text !== originalText);
          }
        }),
      );
    } else {
      extensions.push(EditorState.readOnly.of(true));
    }
    if (langExt) extensions.push(langExt);
    const state = EditorState.create({ doc, extensions });
    const view = new EditorView({ state, parent: containerRef.current });
    return () => {
      view.destroy();
    };
  }, [rawLines, showAnchors, response, langExt, editing, originalText]);

  if (!path) {
    return (
      <div className="flex flex-col items-center justify-center h-full text-[var(--color-text-tertiary)] gap-2">
        <FileCode2 size={36} className="opacity-40" />
        <p className="text-sm">在左侧选择一个文件以查看内容</p>
      </div>
    );
  }

  const showEditButton = !editing && !response?.binary && rawLines.length > 0 && !loading;
  const canSave = editing && dirty;

  return (
    <div className="flex flex-col h-full min-h-0">
      {/* 文件路径栏 */}
      <div className="flex items-center justify-between gap-2 px-4 py-2 border-b border-[var(--color-border)] bg-[var(--color-bg-secondary)]">
        <span className="flex items-center gap-2 text-xs font-mono text-[var(--color-text-secondary)] truncate min-w-0">
          <span className="truncate">{path}</span>
          {editing && dirty && (
            <span
              aria-label="未保存"
              className="shrink-0 flex items-center gap-1 text-xs font-sans text-amber-600 dark:text-amber-400"
            >
              <span className="size-1.5 rounded-full bg-amber-500" />
              未保存
            </span>
          )}
          {editing && (
            <span className="shrink-0 px-1.5 py-0.5 rounded bg-blue-50 dark:bg-blue-900/30 text-blue-700 dark:text-blue-300 text-xs font-sans">
              编辑模式
            </span>
          )}
        </span>
        <span className="flex items-center gap-1.5 shrink-0">
          {editing ? (
            <>
              <button
                type="button"
                onClick={handleSave}
                disabled={!canSave}
                className="flex items-center gap-1 px-2 py-0.5 text-xs rounded border transition-colors disabled:opacity-40 disabled:cursor-not-allowed border-blue-500 bg-blue-600 text-white hover:bg-blue-700"
              >
                <Save size={12} />
                保存
              </button>
              <button
                type="button"
                onClick={handleDiscard}
                className="flex items-center gap-1 px-2 py-0.5 text-xs rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]"
              >
                <RotateCcw size={12} />
                放弃
              </button>
            </>
          ) : (
            <>
              {showEditButton && (
                <button
                  type="button"
                  onClick={handleEnterEdit}
                  disabled={editLoading}
                  className="flex items-center gap-1 px-2 py-0.5 text-xs rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-60"
                >
                  {editLoading ? (
                    <Loader2 size={12} className="animate-spin" />
                  ) : (
                    <Pencil size={12} />
                  )}
                  编辑
                </button>
              )}
              {!response?.binary && rawLines.length > 0 && (
                <button
                  type="button"
                  onClick={() => setShowAnchors((prev) => !prev)}
                  aria-pressed={showAnchors}
                  className={`flex items-center gap-1 px-2 py-0.5 text-xs rounded border transition-colors shrink-0 ${
                    showAnchors
                      ? 'border-blue-500 bg-blue-50 text-blue-700 dark:bg-blue-900/30 dark:text-blue-300'
                      : 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]'
                  }`}
                >
                  <Hash size={12} />
                  显示锚点
                </button>
              )}
            </>
          )}
        </span>
      </div>

      {loading ? (
        <div className="flex items-center justify-center py-10">
          <Loader2 size={20} className="animate-spin text-[var(--color-text-tertiary)]" />
        </div>
      ) : error ? (
        <div
          role="alert"
          className="flex items-center gap-2 p-3 m-4 rounded-lg bg-red-50 dark:bg-red-950 border border-red-200 dark:border-red-800 text-red-700 dark:text-red-300 text-sm"
        >
          <AlertCircle size={16} />
          <span>加载文件失败：{error}</span>
        </div>
      ) : response?.binary ? (
        <div className="p-6 space-y-3">
          <div className="flex items-center gap-2 p-3 rounded-lg bg-[var(--color-bg-secondary)] border border-[var(--color-border)] text-sm">
            <Binary size={16} className="text-[var(--color-text-tertiary)] shrink-0" />
            <span className="text-[var(--color-text-primary)]">
              二进制文件（{response.size ?? 0} 字节）
            </span>
          </div>
          {response.preview && (
            <pre className="p-4 rounded-lg bg-[var(--color-bg-secondary)] border border-[var(--color-border)] text-xs font-mono text-[var(--color-text-secondary)] whitespace-pre-wrap break-all">
              {response.preview}
            </pre>
          )}
        </div>
      ) : (
        <>
          <div ref={containerRef} className="flex-1 min-h-0 overflow-auto text-sm" />
          {/* 状态栏：编辑模式标识 + 行数 + 加载更多 */}
          <div className="flex items-center justify-between px-4 py-1.5 border-t border-[var(--color-border)] bg-[var(--color-bg-secondary)] text-xs text-[var(--color-text-tertiary)]">
            <span>
              {editing ? '编辑模式' : '共 '}
              {editing
                ? `共 ${draftText.split('\n').length} 行`
                : `${response?.total_lines ?? rawLines.length} 行`}
            </span>
            {response?.truncated && !editing && (
              <button
                type="button"
                onClick={handleLoadMore}
                disabled={loadMoreLoading}
                className="flex items-center gap-1 px-2 py-0.5 rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-60"
              >
                {loadMoreLoading && <Loader2 size={12} className="animate-spin" />}
                加载更多
              </button>
            )}
          </div>
        </>
      )}
    </div>
  );
}
