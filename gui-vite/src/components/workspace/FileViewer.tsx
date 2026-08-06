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
} from '@codemirror/view';
import {
  syntaxHighlighting,
  defaultHighlightStyle,
  foldGutter,
  bracketMatching,
  indentOnInput,
} from '@codemirror/language';
import { rust } from '@codemirror/lang-rust';
import { javascript } from '@codemirror/lang-javascript';
import { python } from '@codemirror/lang-python';
import { AlertCircle, Binary, FileCode2, Hash, Loader2 } from 'lucide-react';
import { fetchWorkspaceRead } from '@/lib/api-client';
import type { WorkspaceReadResponse } from '@/lib/types';

const PAGE_LIMIT = 2000;

/**
 * 计算下一次分页请求的 offset（1 起始）：上一窗口起始行 + 窗口内真实内容行数。
 *
 * 优先用响应的 `showing` 窗口（offset + min(limit, total_lines - offset + 1)）。
 * 截断页末尾追加的提示行（"(Showing lines ...)"）不属于内容窗口，不会被计入；
 * 而按显示行数累加会把提示行当成真实行，从第 3 页起每页静默跳过一行。
 * 无 `showing` 时退化为内容行数（截断提示行不计入）。
 */
function computeNextOffset(res: WorkspaceReadResponse): number {
  const showing = res.showing;
  if (showing && res.total_lines !== undefined) {
    const remaining = Math.max(res.total_lines - (showing.offset - 1), 0);
    const windowSize = Math.min(showing.limit, remaining);
    return showing.offset + windowSize;
  }
  const lines = (res.content ?? '').split('\n');
  const realLines = res.truncated ? Math.max(lines.length - 1, 0) : lines.length;
  return (showing?.offset ?? 0) + realLines;
}

/** hashline 前缀：行号 + 短哈希 + 分隔符（"N#ID|"），供 LLM 锚点使用，人类阅读时剥离。 */
const HASHLINE_RE = /^\d+#[0-9a-f]{2}\|/;

/** 剥离单行 hashline 前缀；不匹配的行原样返回。 */
function stripHashline(line: string): string {
  return HASHLINE_RE.test(line) ? line.replace(HASHLINE_RE, '') : line;
}

/** 按文件扩展名选择 CodeMirror 语言扩展；未知类型返回 null（纯文本）。 */
function languageForPath(path: string) {
  const ext = path.split('.').pop()?.toLowerCase() ?? '';
  switch (ext) {
    case 'rs':
      return rust();
    case 'ts':
    case 'tsx':
    case 'mts':
    case 'cts':
      return javascript({ typescript: true });
    case 'js':
    case 'jsx':
    case 'mjs':
    case 'cjs':
      return javascript();
    case 'py':
      return python();
    default:
      return null;
  }
}

interface FileViewerProps {
  /** 当前选中的文件相对路径；null 时显示占位提示。 */
  path: string | null;
}

/** 只读 CodeMirror 6 文件查看器：hashline 剥离、二进制提示、分页加载更多。 */
export default function FileViewer({ path }: FileViewerProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [response, setResponse] = useState<WorkspaceReadResponse | null>(null);
  const [rawLines, setRawLines] = useState<string[]>([]);
  const [nextOffset, setNextOffset] = useState(0);
  const [showAnchors, setShowAnchors] = useState(false);
  const [loadMoreLoading, setLoadMoreLoading] = useState(false);

  const loadFile = useCallback(async (filePath: string) => {
    setLoading(true);
    setError(null);
    setResponse(null);
    setRawLines([]);
    setNextOffset(0);
    setShowAnchors(false);
    try {
      const res = await fetchWorkspaceRead(filePath, undefined, PAGE_LIMIT);
      setResponse(res);
      if (res.content !== undefined) {
        const lines = res.content.split('\n');
        setRawLines(lines);
        setNextOffset(computeNextOffset(res));
      }
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : '加载失败');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (path) {
      loadFile(path);
    } else {
      setResponse(null);
      setRawLines([]);
      setNextOffset(0);
    }
  }, [path, loadFile]);

  const handleLoadMore = async () => {
    if (!path) return;
    setLoadMoreLoading(true);
    try {
      const res = await fetchWorkspaceRead(path, nextOffset, PAGE_LIMIT);
      setResponse((prev) => ({ ...prev, ...res }));
      if (res.content !== undefined) {
        const lines = res.content.split('\n');
        setRawLines((prev) => [...prev, ...lines]);
        setNextOffset(computeNextOffset(res));
      }
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : '加载失败');
    } finally {
      setLoadMoreLoading(false);
    }
  };

  const langExt = useMemo(() => (path ? languageForPath(path) : null), [path]);

  // CodeMirror 实例：内容/语言/锚点开关变化时重建
  useEffect(() => {
    if (!containerRef.current || response?.binary) return;
    const doc = showAnchors ? rawLines.join('\n') : rawLines.map(stripHashline).join('\n');
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
      EditorState.readOnly.of(true),
    ];
    if (langExt) extensions.push(langExt);
    const state = EditorState.create({ doc, extensions });
    const view = new EditorView({ state, parent: containerRef.current });
    return () => {
      view.destroy();
    };
  }, [rawLines, showAnchors, response, langExt]);

  if (!path) {
    return (
      <div className="flex flex-col items-center justify-center h-full text-[var(--color-text-tertiary)] gap-2">
        <FileCode2 size={36} className="opacity-40" />
        <p className="text-sm">在左侧选择一个文件以查看内容</p>
      </div>
    );
  }

  return (
    <div className="flex flex-col h-full min-h-0">
      {/* 文件路径栏 */}
      <div className="flex items-center justify-between px-4 py-2 border-b border-[var(--color-border)] bg-[var(--color-bg-secondary)]">
        <span className="text-xs font-mono text-[var(--color-text-secondary)] truncate">
          {path}
        </span>
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
          {/* 状态栏：行数 + 加载更多 */}
          <div className="flex items-center justify-between px-4 py-1.5 border-t border-[var(--color-border)] bg-[var(--color-bg-secondary)] text-xs text-[var(--color-text-tertiary)]">
            <span>共 {response?.total_lines ?? rawLines.length} 行</span>
            {response?.truncated && (
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
