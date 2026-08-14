import { useCallback, useEffect, useMemo, useState } from 'react';
import { Tree, type NodeRendererProps } from 'react-arborist';
import { ChevronRight, File, Folder, Loader2, RefreshCw } from 'lucide-react';
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
  /** 树容器高度（react-arborist 需要显式数值高度）。 */
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
}

function WorkspaceNode({
  node,
  style,
  dragHandle,
  onSelectFile,
  onExpandDir,
  expandingPath,
}: NodeRendererProps<WorkspaceNodeData> & NodeExtraProps) {
  const isDir = node.data.type === 'dir';
  const isLoading = isDir && expandingPath === node.data.path;

  const handleToggle = () => {
    if (isDir && !node.isOpen) {
      onExpandDir(node.data.path);
    }
    node.toggle();
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
      ) : (
        <span className="w-5 shrink-0" />
      )}
      {isDir ? (
        <Folder size={14} className="shrink-0 text-yellow-500" />
      ) : (
        <File size={14} className="shrink-0 text-[var(--color-text-tertiary)]" />
      )}
      <span className="truncate min-w-0 text-[var(--color-text-primary)]">{node.data.name}</span>
    </div>
  );
}

/**
 * 工作区文件树（react-arborist）。
 *
 * 目录子条目懒加载：展开目录时调用 GET /workspace/tree?path=&depth=1，
 * 结果写入 childrenMap 状态，react-arborist 通过 childrenAccessor 读取。
 * 目录未加载时返回空数组（保持可展开），加载完成后子条目自动出现。
 */
export default function WorkspaceTree({
  onSelectFile,
  height = DEFAULT_HEIGHT,
  sessionId,
}: WorkspaceTreeProps) {
  const [rootEntries, setRootEntries] = useState<WorkspaceEntry[]>([]);
  const [childrenMap, setChildrenMap] = useState<Record<string, WorkspaceNodeData[]>>({});
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [expandingPath, setExpandingPath] = useState<string | null>(null);

  const loadRoot = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const res = await fetchWorkspaceTree('', 1, sessionId);
      setRootEntries(res.entries);
      setChildrenMap({});
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : '加载失败');
    } finally {
      setLoading(false);
    }
  }, [sessionId]);

  useEffect(() => {
    loadRoot();
  }, [loadRoot]);

  const loadChildren = useCallback(
    async (path: string) => {
      setExpandingPath(path);
      try {
        const res = await fetchWorkspaceTree(path, 1, sessionId);
        setChildrenMap((prev) => ({ ...prev, [path]: res.entries.map(toNode) }));
      } catch {
        // 子目录加载失败：保持空展开状态，用户可再次点击重试
        setChildrenMap((prev) => ({ ...prev, [path]: [] }));
      } finally {
        setExpandingPath(null);
      }
    },
    [sessionId],
  );

  const data = useMemo(() => rootEntries.map(toNode), [rootEntries]);

  // 目录返回其已加载子条目（未加载为空数组 → 仍可展开）；文件为叶子节点
  const accessChildren = useCallback(
    (d: WorkspaceNodeData) => (d.type === 'dir' ? (childrenMap[d.path] ?? []) : null),
    [childrenMap],
  );

  if (loading) {
    return (
      <div className="flex items-center justify-center py-10">
        <Loader2 size={20} className="animate-spin text-[var(--color-text-tertiary)]" />
      </div>
    );
  }

  if (error) {
    return (
      <div className="flex flex-col items-center gap-2 py-10 px-4 text-center">
        <p role="alert" className="text-sm text-red-600 dark:text-red-400">
          加载工作区失败：{error}
        </p>
        <button
          type="button"
          onClick={loadRoot}
          className="flex items-center gap-1 px-3 py-1 text-xs rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]"
        >
          <RefreshCw size={12} />
          重试
        </button>
      </div>
    );
  }

  if (rootEntries.length === 0) {
    return (
      <div className="flex items-center justify-center py-10">
        <p className="text-sm text-[var(--color-text-tertiary)]">工作区为空</p>
      </div>
    );
  }

  return (
    <Tree<WorkspaceNodeData>
      data={data}
      childrenAccessor={accessChildren}
      width="100%"
      height={height}
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
        />
      )}
    </Tree>
  );
}
