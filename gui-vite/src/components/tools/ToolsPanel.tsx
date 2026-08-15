import { useState, useEffect } from 'react';
import { apiGet } from '@/lib/api-client';
import type { ListToolsResponse, ToolInfo } from '@/lib/types';
import { Wrench, Loader2, AlertCircle, ChevronRight, Package } from 'lucide-react';

/**
 * 工具面板：展示全部系统工具（与 LLM 收到的 tools 列表同源）。
 *
 * 工具 = 原子化操作原语（系统内置），与"技能"（方法论）是两套概念：
 * 技能面板展示 GEPA 学习方法论，工具面板展示可调用的原子操作。
 */
export default function ToolsPanel() {
  const [tools, setTools] = useState<ToolInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<ToolInfo | null>(null);

  useEffect(() => {
    let cancelled = false;

    async function load() {
      setLoading(true);
      setError(null);
      try {
        const res = await apiGet<ListToolsResponse>('/tools');
        if (!cancelled) {
          setTools(res.tools);
        }
      } catch (err) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : '加载工具失败');
        }
      } finally {
        if (!cancelled) {
          setLoading(false);
        }
      }
    }

    load();
    return () => {
      cancelled = true;
    };
  }, []);

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
        </div>

        <div className="flex-1 overflow-y-auto p-3">
          {loading ? (
            <div
              className="flex items-center justify-center py-16"
              aria-live="polite"
              aria-label="正在加载工具"
            >
              <Loader2 size={24} className="animate-spin text-[var(--color-text-tertiary)]" />
            </div>
          ) : error ? (
            <div
              role="alert"
              className="flex items-center gap-2 p-3 rounded-lg bg-red-50 dark:bg-red-950 border border-red-200 dark:border-red-800 text-red-700 dark:text-red-300 text-sm"
            >
              <AlertCircle size={16} />
              <span>{error}</span>
            </div>
          ) : tools.length === 0 ? (
            <div className="flex flex-col items-center justify-center py-16 text-[var(--color-text-tertiary)]">
              <Package size={40} className="mb-3 opacity-40" />
              <p className="text-sm">暂无可用工具</p>
            </div>
          ) : (
            <div className="space-y-1">
              {tools.map((tool) => (
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
