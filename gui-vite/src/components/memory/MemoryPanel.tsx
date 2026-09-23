import { useState, useEffect, useMemo } from 'react';
import { fetchMemories } from '@/lib/api-client';
import { useResource } from '@/hooks/use-resource';
import { usePolling } from '@/hooks/use-polling';
import type { MemoryEntry } from '@/lib/types';
import { buildMemoryTree, countLeaves, type MemoryTreeNode } from '@/lib/memory-tree';
import { MemoryStick, Folder, Star, RefreshCw, ChevronRight, ChevronDown } from 'lucide-react';
import { Spinner } from '@/components/ui/Spinner';
import { ErrorBanner } from '@/components/ui/ErrorBanner';
import { renderIndexOrText } from '@/lib/vfs-index';

/** 内容层级（简介 / 目录 / 正文；ADR-001 修订：L1 语义 = 目录）。 */
const LEVELS = [
  { key: 'L0', label: '简介', pick: (e: MemoryEntry) => e.abstract },
  { key: 'L1', label: '目录', pick: (e: MemoryEntry) => e.overview },
  { key: 'L2', label: '正文', pick: (e: MemoryEntry) => e.detail },
] as const;
type LevelKey = (typeof LEVELS)[number]['key'];

/** 默认层级：有内容的最高层（detail → overview → abstract）。 */
function defaultLevel(entry: MemoryEntry): LevelKey | null {
  if (entry.detail !== null) return 'L2';
  if (entry.overview !== null) return 'L1';
  if (entry.abstract !== null) return 'L0';
  return null;
}

/** 记忆面板：左侧分类树 + 右侧详情（简介/目录/正文 层级可切换）。 */
export default function MemoryPanel() {
  const [selected, setSelected] = useState<MemoryEntry | null>(null);
  const [level, setLevel] = useState<LevelKey | null>(null);
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const { data, loading, error, reload, silentReload } = useResource(() => fetchMemories(), [], {
    errorFallback: '加载失败',
  });
  // 自动轮询（8s）：记忆在 agent 运行时持续更新，静默刷新不闪 Spinner
  usePolling(() => void silentReload(), 8000, { immediate: false });
  // 派生数组 useMemo 化：useEffect deps 需要稳定引用（?? [] 每次渲染新建数组）
  const memories = useMemo(() => data?.memories ?? [], [data]);
  // 分类树：目录字母序 + 类目内时间倒排（后端顺序的子序列）
  const tree = useMemo(() => buildMemoryTree(memories), [memories]);

  // 刷新后重定位选中项（按 uri；已消失则清空选择）
  useEffect(() => {
    setSelected((prev) => (prev ? (memories.find((m) => m.uri === prev.uri) ?? null) : null));
  }, [memories]);

  const handleSelect = (entry: MemoryEntry) => {
    setSelected(entry);
    setLevel(defaultLevel(entry));
  };

  const toggleDir = (path: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };

  const activeContent =
    selected && level ? (LEVELS.find((l) => l.key === level)?.pick(selected) ?? null) : null;

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="flex items-center justify-between px-6 py-4 border-b border-[var(--color-border)] shrink-0">
        <h2 className="text-lg font-semibold text-[var(--color-text-primary)]">记忆</h2>
        <button
          onClick={() => void reload()}
          disabled={loading}
          className="flex items-center gap-1 px-2 py-1 text-xs rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50"
        >
          <RefreshCw size={12} />
          刷新
        </button>
      </div>

      {/* Body：双栏（左分类树 / 右详情） */}
      <div className="flex flex-1 min-h-0 overflow-hidden">
        {/* 左：分类树 */}
        <div className="w-[40%] min-w-[260px] max-w-[360px] flex flex-col border-r border-[var(--color-border)]">
          <div className="flex-1 overflow-y-auto p-3">
            {error && <ErrorBanner message={error} />}

            {!error && loading && <Spinner />}

            {!error && !loading && memories.length === 0 && (
              <div className="flex flex-col items-center justify-center py-16 text-[var(--color-text-tertiary)]">
                <MemoryStick size={40} className="mb-3 opacity-40" />
                <p className="text-sm">暂无记忆</p>
              </div>
            )}

            {!error && !loading && memories.length > 0 && (
              <div className="space-y-0.5">
                {tree.map((node) => (
                  <TreeNodeRow
                    key={node.path}
                    node={node}
                    depth={0}
                    collapsed={collapsed}
                    onToggleDir={toggleDir}
                    selectedUri={selected?.uri ?? null}
                    onSelect={handleSelect}
                  />
                ))}
              </div>
            )}
          </div>
        </div>

        {/* 右：选中条目详情（层级切换） */}
        <div className="flex-1 flex flex-col overflow-hidden">
          {!selected ? (
            <div className="flex-1 flex flex-col items-center justify-center text-[var(--color-text-tertiary)]">
              <MemoryStick size={48} className="mb-4 opacity-30" />
              <p className="text-sm">选择一条记忆查看内容</p>
            </div>
          ) : (
            <div className="flex-1 overflow-y-auto p-6">
              <h4 className="text-sm font-medium text-[var(--color-text-primary)] truncate mb-2">
                {selected.name ?? selected.uri}
              </h4>
              <p className="text-xs text-[var(--color-text-tertiary)] mb-3 break-all">
                {selected.uri}
              </p>
              {/* 层级切换：三层均显式可见（无内容的层级禁用） */}
              <div className="flex items-center gap-1 mb-3" role="tablist" aria-label="内容层级">
                {LEVELS.map((lv) => {
                  const has = lv.pick(selected) !== null;
                  const active = level === lv.key;
                  return (
                    <button
                      key={lv.key}
                      role="tab"
                      aria-selected={active}
                      disabled={!has}
                      onClick={() => setLevel(lv.key)}
                      className={`px-2 py-0.5 text-xs rounded border transition-colors cursor-pointer disabled:cursor-not-allowed disabled:opacity-40 ${
                        active
                          ? 'border-blue-300 bg-blue-50 text-blue-700 dark:border-blue-700 dark:bg-blue-900/30 dark:text-blue-300'
                          : 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]'
                      }`}
                    >
                      {lv.label}
                    </button>
                  );
                })}
              </div>
              {activeContent !== null ? (
                <pre className="overflow-y-auto p-4 rounded-lg bg-[var(--color-bg-secondary)] text-sm text-[var(--color-text-secondary)] whitespace-pre-wrap font-mono leading-relaxed border border-[var(--color-border)]">
                  {renderIndexOrText(activeContent) || '(空内容)'}
                </pre>
              ) : (
                <p className="text-sm text-[var(--color-text-tertiary)] py-8 text-center">
                  该条目暂无内容
                </p>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

/** 树行（递归）：目录可折叠（显示叶子计数），叶子可选（显示 importance/标签）。 */
function TreeNodeRow({
  node,
  depth,
  collapsed,
  onToggleDir,
  selectedUri,
  onSelect,
}: {
  node: MemoryTreeNode;
  depth: number;
  collapsed: Set<string>;
  onToggleDir: (path: string) => void;
  selectedUri: string | null;
  onSelect: (entry: MemoryEntry) => void;
}) {
  const indent = { paddingLeft: `${depth * 14 + 8}px` };

  if (node.kind === 'dir') {
    const isCollapsed = collapsed.has(node.path);
    return (
      <div>
        <button
          onClick={() => onToggleDir(node.path)}
          className="w-full text-left flex items-center gap-1.5 py-1 rounded-md text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] cursor-pointer transition-colors"
          style={indent}
        >
          {isCollapsed ? (
            <ChevronRight size={14} className="shrink-0" />
          ) : (
            <ChevronDown size={14} className="shrink-0" />
          )}
          <Folder size={15} className="shrink-0 text-yellow-500" />
          <span className="text-sm truncate">{node.name}</span>
          <span className="text-xs text-[var(--color-text-tertiary)] shrink-0">
            ({countLeaves(node)})
          </span>
        </button>
        {!isCollapsed &&
          node.children.map((child) => (
            <TreeNodeRow
              key={child.path}
              node={child}
              depth={depth + 1}
              collapsed={collapsed}
              onToggleDir={onToggleDir}
              selectedUri={selectedUri}
              onSelect={onSelect}
            />
          ))}
      </div>
    );
  }

  const { entry } = node;
  const active = selectedUri === entry.uri;
  return (
    <button
      onClick={() => onSelect(entry)}
      className={`w-full text-left flex items-center justify-between py-1.5 pr-2 rounded-md cursor-pointer transition-colors ${
        active
          ? 'bg-blue-50 dark:bg-blue-900/20 border border-blue-200 dark:border-blue-800'
          : 'border border-transparent hover:bg-[var(--color-bg-hover)]'
      }`}
      style={indent}
      title={entry.uri}
    >
      <div className="flex items-center gap-2 min-w-0">
        <MemoryStick size={15} className="shrink-0 text-[var(--color-text-tertiary)]" />
        <span className="text-sm text-[var(--color-text-primary)] truncate">{node.name}</span>
      </div>
      <div className="flex items-center gap-2 shrink-0">
        {entry.metadata.importance > 0 && (
          <span className="flex items-center gap-1 text-xs text-[var(--color-text-tertiary)]">
            <Star size={12} className="text-yellow-500" />
            {Math.round(entry.metadata.importance * 100)}%
          </span>
        )}
        {entry.metadata.tags.slice(0, 2).map((tag) => (
          <span
            key={tag}
            className="px-1.5 py-0.5 text-xs rounded bg-[var(--color-bg-secondary)] text-[var(--color-text-secondary)]"
          >
            {tag}
          </span>
        ))}
      </div>
    </button>
  );
}
