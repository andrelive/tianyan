import { useState, useEffect, useMemo } from 'react';
import { fetchMemories } from '@/lib/api-client';
import { useResource } from '@/hooks/use-resource';
import { usePolling } from '@/hooks/use-polling';
import type { MemoryEntry } from '@/lib/types';
import { MemoryStick, Folder, Star, RefreshCw } from 'lucide-react';
import { Spinner } from '@/components/ui/Spinner';
import { ErrorBanner } from '@/components/ui/ErrorBanner';

/** 按 detail → overview → abstract 三级回退取内容，返回内容与对应层级标签。 */
function resolveContent(entry: MemoryEntry): { content: string; level: string } | null {
  if (entry.detail !== null) return { content: entry.detail, level: 'L2 详情' };
  if (entry.overview !== null) return { content: entry.overview, level: 'L1 概览' };
  if (entry.abstract !== null) return { content: entry.abstract, level: 'L0 摘要' };
  return null;
}

/** 记忆面板：左侧列表 + 右侧详情双栏（与工具/技能/角色面板一致）。 */
export default function MemoryPanel() {
  const [selected, setSelected] = useState<MemoryEntry | null>(null);
  const { data, loading, error, reload, silentReload } = useResource(() => fetchMemories(), [], {
    errorFallback: '加载失败',
  });
  // 自动轮询（8s）：记忆在 agent 运行时持续更新，静默刷新不闪 Spinner
  usePolling(() => void silentReload(), 8000, { immediate: false });
  // 派生数组 useMemo 化：useEffect deps 需要稳定引用（?? [] 每次渲染新建数组）
  const memories = useMemo(() => data?.memories ?? [], [data]);

  // 刷新后重定位选中项（按 uri；已消失则清空选择）
  useEffect(() => {
    setSelected((prev) => (prev ? (memories.find((m) => m.uri === prev.uri) ?? null) : null));
  }, [memories]);

  const selectedContent = selected ? resolveContent(selected) : null;

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

      {/* Body：双栏（左列表 / 右详情） */}
      <div className="flex flex-1 min-h-0 overflow-hidden">
        {/* 左：记忆列表 */}
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
              <div className="space-y-1">
                {memories.map((entry) => (
                  <button
                    key={entry.uri}
                    onClick={() => setSelected(entry)}
                    className={`w-full text-left flex items-center justify-between px-3 py-2 rounded-md cursor-pointer transition-colors ${
                      selected?.uri === entry.uri
                        ? 'bg-blue-50 dark:bg-blue-900/20 border border-blue-200 dark:border-blue-800'
                        : 'border border-transparent hover:bg-[var(--color-bg-hover)]'
                    }`}
                  >
                    <div className="flex items-center gap-2 min-w-0">
                      {entry.is_directory ? (
                        <Folder size={16} className="shrink-0 text-yellow-500" />
                      ) : (
                        <MemoryStick size={16} className="shrink-0 text-[var(--color-text-tertiary)]" />
                      )}
                      <span className="text-sm text-[var(--color-text-primary)] truncate">
                        {entry.name ?? entry.metadata.uri.uri}
                      </span>
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
                ))}
              </div>
            )}
          </div>
        </div>

        {/* 右：选中条目详情 */}
        <div className="flex-1 flex flex-col overflow-hidden">
          {!selected ? (
            <div className="flex-1 flex flex-col items-center justify-center text-[var(--color-text-tertiary)]">
              <MemoryStick size={48} className="mb-4 opacity-30" />
              <p className="text-sm">选择一条记忆查看内容</p>
            </div>
          ) : (
            <div className="flex-1 overflow-y-auto p-6">
              <div className="flex items-center justify-between mb-2">
                <h4 className="text-sm font-medium text-[var(--color-text-primary)] truncate max-w-[70%]">
                  {selected.name ?? selected.metadata.uri.uri}
                </h4>
                {selectedContent && (
                  <span className="px-2 py-0.5 text-xs rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] shrink-0">
                    {selectedContent.level}
                  </span>
                )}
              </div>
              <p className="text-xs text-[var(--color-text-tertiary)] mb-3 break-all">{selected.uri}</p>
              {selectedContent ? (
                <pre className="overflow-y-auto p-4 rounded-lg bg-[var(--color-bg-secondary)] text-sm text-[var(--color-text-secondary)] whitespace-pre-wrap font-mono leading-relaxed border border-[var(--color-border)]">
                  {selectedContent.content || '(空内容)'}
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
