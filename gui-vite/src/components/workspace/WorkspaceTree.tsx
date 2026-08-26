import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Tree, type NodeRendererProps } from 'react-arborist';
import { useResource } from '@/hooks/use-resource';
import { AlertCircle, ChevronRight, File, Folder, Loader2, RefreshCw } from 'lucide-react';
import { fetchWorkspaceTree } from '@/lib/api-client';
import type { WorkspaceEntry } from '@/lib/types';

/** react-arborist 节点数据（id = 相对路径）。 */
export interface WorkspaceNodeData {
  id: string;
  name: string;
  path: string;
  type: 'dir' | 'file';
  size?: number;
}

interface WorkspaceTreeProps {
  /** 点击文件时回调（参数为相对路径）。 */
  onSelectFile: (path: string) => void;
  /** 树容器高度兜底（react-arborist 需要显式数值高度；默认自适应容器）。 */
  height?: number;
  /** 会话 ID（解析会话绑定的工作目录；缺省 = 全局配置）。 */
  sessionId?: string;
}

const DEFAULT_HEIGHT = 600;

function toNode(entry: WorkspaceEntry): WorkspaceNodeData {
  return {
    id: entry.path,
    name: entry.name,
    path: entry.path,
    type: entry.type,
    size: entry.size,
  };
}

interface NodeExtraProps {
  onSelectFile: (path: string) => void;
  onExpandDir: (path: string) => void;
  expandingPath: string | null;
  /** 子目录加载失败路径 → 错误信息（用于行内失败反馈与重试）。 */
  loadErrors: Record<string, string>;
  onRetryDir: (path: string) => void;
}

function WorkspaceNode({
  node,
  style,
  dragHandle,
  onSelectFile,
  onExpandDir,
  expandingPath,
  loadErrors,
  onRetryDir,
}: NodeRendererProps<WorkspaceNodeData> & NodeExtraProps) {
  const isDir = node.data.type === 'dir';
  const isLoading = isDir && expandingPath === node.data.path;
  const loadFailed = isDir && !!loadErrors[node.data.path];

  const handleToggle = () => {
    if (isDir && !node.isOpen) {
      onExpandDir(node.data.path);
    }
    node.toggle();
  };

  const handleRetry = (e: React.MouseEvent) => {
    e.stopPropagation();
    onRetryDir(node.data.path);
  };

  const handleRowClick = () => {
    if (isDir) {
      handleToggle();
      return;
    }
    node.select();
    onSelectFile(node.data.path);
  };

  return (
    <div
      role="treeitem"
      aria-selected={node.isSelected}
      aria-expanded={isDir ? node.isOpen : undefined}
      style={style}
      ref={dragHandle}
      onClick={handleRowClick}
      className={`flex items-center gap-1.5 pr-2 text-sm cursor-pointer select-none transition-colors ${
        node.isSelected
          ? 'bg-blue-50 text-blue-700 dark:bg-blue-900/30 dark:text-blue-300'
          : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]'
      }`}
    >
      {isDir ? (
        loadFailed ? (
          <button
            type="button"
            onClick={handleRetry}
            title={`${loadErrors[node.data.path] ?? '目录加载失败'}，点击重试`}
            aria-label={`重试加载 ${node.data.name}`}
            className="w-5 h-5 shrink-0 flex items-center justify-center rounded text-red-500 hover:bg-red-100 dark:hover:bg-red-900/30"
          >
            <RefreshCw size={13} />
          </button>
        ) : (
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation();
              handleToggle();
            }}
            aria-label={node.isOpen ? `收起 ${node.data.name}` : `展开 ${node.data.name}`}
            className="w-5 h-5 shrink-0 flex items-center justify-center rounded hover:bg-[var(--color-bg-tertiary)] text-[var(--color-text-tertiary)]"
          >
            {isLoading ? (
              <Loader2 size={13} className="animate-spin" />
            ) : (
              <ChevronRight
                size={14}
                className={`transition-transform ${node.isOpen ? 'rotate-90' : ''}`}
              />
            )}
          </button>
        )
      ) : (
        <span className="w-5 shrink-0" />
      )}
      {isDir ? (
        <Folder size={14} className="shrink-0 text-yellow-500" />
      ) : (
        <File size={14} className="shrink-0 text-[var(--color-text-tertiary)]" />
      )}
      <span className={`truncate min-w-0 ${loadFailed ? 'text-red-600 dark:text-red-400' : 'text-[var(--color-text-primary)]'}`}>
        {node.data.name}
      </span>
      {loadFailed && <AlertCircle size={12} className="shrink-0 text-red-500" />}
    </div>
  );
}

/**
 * 工作区文件树（react-arborist）。
 *
 * 目录子条目懒加载：展开目录时调用 GET /workspace/tree?path=&depth=1，
 * 结果写入 childrenMap 状态，react-arborist 通过 childrenAccessor 读取。
 * 目录加载失败会在行内显示红色重试按钮（P0：消除"spinner 后变空且无提示"）。
 * 树高度通过 ResizeObserver 自适应容器（P0：不再固定 600px 被裁剪/留白）。
 */
export default function WorkspaceTree({
  onSelectFile,
  height = DEFAULT_HEIGHT,
  sessionId,
}: WorkspaceTreeProps) {
  const [childrenMap, setChildrenMap] = useState<Record<string, WorkspaceNodeData[]>>({});
  const [expandingPath, setExpandingPath] = useState<string | null>(null);
  const [loadErrors, setLoadErrors] = useState<Record<string, string>>({});
  const containerRef = useRef<HTMLDivElement>(null);
  const [containerHeight, setContainerHeight] = useState<number | null>(null);

  // 自适应高度：跟随容器（flex-1 min-h-0）而非固定 600px；
  // 无 ResizeObserver 环境（jsdom 测试）回退默认高度。
  useEffect(() => {
    const el = containerRef.current;
    if (!el || typeof ResizeObserver === 'undefined') return;
    const update = () => setContainerHeight(el.clientHeight);
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // 根加载（useResource：加载/错误/竞态收敛；sessionId 变化自动重取）
  const {
    data: rootResp,
    loading,
    error,
    reload,
  } = useResource(
    async () => {
      const res = await fetchWorkspaceTree('', 1, sessionId);
      setChildrenMap({});
      setLoadErrors({});
      return res;
    },
    [sessionId],
    { errorFallback: '加载失败' },
  );
  // 派生数组 useMemo 化：useMemo 的 data 依赖需要稳定引用（?? [] 每次渲染新建数组）
  const rootEntries = useMemo(() => rootResp?.entries ?? [], [rootResp]);

  const loadChildren = useCallback(
    async (path: string) => {
      setExpandingPath(path);
      try {
        const res = await fetchWorkspaceTree(path, 1, sessionId);
        setChildrenMap((prev) => ({ ...prev, [path]: res.entries.map(toNode) }));
        // 成功即清除该目录的失败标记
        setLoadErrors((prev) => {
          if (!prev[path]) return prev;
          const next = { ...prev };
          delete next[path];
          return next;
        });
      } catch (err: unknown) {
        // 子目录加载失败：记录错误 → 行内红色提示 + 重试按钮（不再静默变空）
        const msg = err instanceof Error ? err.message : '加载失败';
        setChildrenMap((prev) => ({ ...prev, [path]: [] }));
        setLoadErrors((prev) => ({ ...prev, [path]: msg }));
      } finally {
        setExpandingPath(null);
      }
    },
    [sessionId],
  );

  const onRetryDir = useCallback(
    (path: string) => {
      setLoadErrors((prev) => {
        const next = { ...prev };
        delete next[path];
        return next;
      });
      loadChildren(path);
    },
    [loadChildren],
  );

  const data = useMemo(() => rootEntries.map(toNode), [rootEntries]);

  // 目录返回其已加载子条目（未加载为空数组 → 仍可展开）；文件为叶子节点
  const accessChildren = useCallback(
    (d: WorkspaceNodeData) => (d.type === 'dir' ? (childrenMap[d.path] ?? []) : null),
    [childrenMap],
  );

  return (
    <div ref={containerRef} className="h-full">
      {loading ? (
        <div className="flex items-center justify-center py-10">
          <Loader2 size={20} className="animate-spin text-[var(--color-text-tertiary)]" />
        </div>
      ) : error ? (
        <div className="flex flex-col items-center gap-2 py-10 px-4 text-center">
          <p role="alert" className="text-sm text-red-600 dark:text-red-400">
            加载目录失败：{error}
          </p>
          <button
            type="button"
            onClick={reload}
            className="flex items-center gap-1 px-3 py-1 text-xs rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]"
          >
            <RefreshCw size={12} />
            重试
          </button>
        </div>
      ) : rootEntries.length === 0 ? (
        <div className="flex items-center justify-center py-10">
          <p className="text-sm text-[var(--color-text-tertiary)]">目录为空</p>
        </div>
      ) : (
        <Tree<WorkspaceNodeData>
          data={data}
          childrenAccessor={accessChildren}
          width="100%"
          height={containerHeight ?? height}
          rowHeight={28}
          indent={16}
          openByDefault={false}
          disableDrag
          disableDrop
          disableEdit
          aria-label="工作区文件树"
        >
          {(props) => (
            <WorkspaceNode
              {...props}
              onSelectFile={onSelectFile}
              onExpandDir={loadChildren}
              expandingPath={expandingPath}
              loadErrors={loadErrors}
              onRetryDir={onRetryDir}
            />
          )}
        </Tree>
      )}
    </div>
  );
}
