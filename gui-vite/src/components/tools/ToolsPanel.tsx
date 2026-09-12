import { useState } from 'react';
import { getTools } from '@/lib/api-client';
import type { ListToolsResponse, ToolInfo } from '@/lib/types';
import { useResource } from '@/hooks/use-resource';
import { Wrench, ChevronRight, Package, Search } from 'lucide-react';
import { Spinner } from '@/components/ui/Spinner';
import { ErrorBanner } from '@/components/ui/ErrorBanner';
import { EmptyState } from '@/components/ui/EmptyState';

/**
 * 工具面板：展示全部系统工具（与 LLM 收到的 tools 列表同源）。
 *
 * 工具 = 原子化操作原语（系统内置），与"技能"（方法论）是两套概念：
 * 技能面板展示 GEPA 学习方法论，工具面板展示可调用的原子操作。
 */
export default function ToolsPanel() {
  const [selected, setSelected] = useState<ToolInfo | null>(null);
  const [query, setQuery] = useState('');
  const { data, loading, error } = useResource<ListToolsResponse>(() => getTools(), [], {
    errorFallback: '加载工具失败',
  });
  const tools = data?.tools ?? [];
  const q = query.trim().toLowerCase();
  const filtered = q
    ? tools.filter(
        (t) => t.name.toLowerCase().includes(q) || (t.description ?? '').toLowerCase().includes(q),
      )
    : tools;

  return (
    <div className="flex h-full">
      {/* Left panel: tool list */}
      <div className="w-[40%] min-w-[260px] max-w-[360px] flex flex-col border-r border-[var(--color-border)]">
        <div className="px-4 py-4 border-b border-[var(--color-border)]">
          <h2 className="text-lg font-semibold text-[var(--color-text-primary)] flex items-center gap-2">
            <Wrench size={20} />
            工具
          </h2>
          <p className="text-xs text-[var(--color-text-tertiary)] mt-1">
            系统内置原子操作（{tools.length} 个），LLM 通过 tool call 调用
          </p>
          <div className="relative mt-3">
            <Search
              size={14}
              className="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--color-text-tertiary)]"
            />
            <input
              type="text"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="搜索工具..."
              aria-label="搜索工具"
              className="w-full pl-8 pr-3 py-1.5 text-xs rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-tertiary)] focus:outline-none focus:ring-2 focus:ring-blue-500/30 focus:border-blue-500"
            />
          </div>
        </div>

        <div className="flex-1 overflow-y-auto p-3">
          {loading ? (
            <Spinner label="正在加载工具" />
          ) : error ? (
            <ErrorBanner message={error} />
          ) : tools.length === 0 ? (
            <EmptyState icon={Package} title="暂无可用工具" />
          ) : filtered.length === 0 ? (
            <EmptyState icon={Search} title="未找到匹配工具" />
          ) : (
            <div className="space-y-1">
              {filtered.map((tool) => (
                <button
                  key={tool.name}
                  onClick={() => setSelected(tool)}
                  className={`w-full text-left px-3 py-2.5 rounded-lg text-sm transition-colors ${
                    selected?.name === tool.name
                      ? 'bg-blue-50 text-blue-700 dark:bg-blue-900/30 dark:text-blue-300'
                      : 'text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)]'
                  }`}
                >
                  <div className="flex items-center justify-between">
                    <div className="min-w-0 flex-1">
                      <code className="font-mono text-sm font-medium">{tool.name}</code>
                      <p className="text-xs text-[var(--color-text-tertiary)] truncate mt-0.5">
                        {tool.description}
                      </p>
                    </div>
                    {selected?.name === tool.name && (
                      <ChevronRight size={14} className="shrink-0 ml-2 text-blue-500" />
                    )}
                  </div>
                </button>
              ))}
            </div>
          )}
        </div>
      </div>

      {/* Right panel: tool detail */}
      <div className="flex-1 flex flex-col overflow-hidden">
        {!selected ? (
          <div className="flex-1 flex flex-col items-center justify-center text-[var(--color-text-tertiary)]">
            <Wrench size={48} className="mb-4 opacity-30" />
            <p className="text-sm">选择一个工具查看详情</p>
          </div>
        ) : (
          <>
            <div className="px-6 py-4 border-b border-[var(--color-border)]">
              <h2 className="text-lg font-semibold text-[var(--color-text-primary)] font-mono">
                {selected.name}
              </h2>
              <p className="text-sm text-[var(--color-text-secondary)] mt-1">
                {selected.description}
              </p>
            </div>

            <div className="flex-1 overflow-y-auto p-6">
              <h3 className="text-sm font-medium text-[var(--color-text-primary)] mb-2">
                参数 Schema
              </h3>
              <pre className="p-4 rounded-lg bg-[var(--color-bg-secondary)] border border-[var(--color-border)] text-xs font-mono text-[var(--color-text-secondary)] overflow-auto max-h-[70%] whitespace-pre-wrap break-words">
                {JSON.stringify(selected.parameters, null, 2)}
              </pre>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
