import { useState, useEffect, useCallback } from 'react';
import { fetchRetrievalTraces } from '@/lib/api-client';
import type { RetrievalTrace, RetrievalStepType } from '@/lib/types';
import { Loader2, AlertCircle, Route, RefreshCw, Clock, Zap, FileText } from 'lucide-react';

const STEP_TYPE_LABELS: Record<RetrievalStepType, string> = {
  intent_analysis: '意图分析',
  l0_search: 'L0 搜索',
  l1_search: 'L1 搜索',
  content_load: '内容加载',
  aggregation: '聚合',
};

/** 毫秒 → 人类可读耗时。 */
function formatDuration(ms: number): string {
  if (ms >= 1000) return `${(ms / 1000).toFixed(2)}s`;
  return `${ms}ms`;
}

/** RFC3339 时间戳 → 本地时间（HH:mm:ss）。 */
function formatTime(ts: string): string {
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return ts;
  return d.toLocaleTimeString('zh-CN', { hour12: false });
}

export default function RetrievalTracesPanel() {
  const [traces, setTraces] = useState<RetrievalTrace[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<RetrievalTrace | null>(null);

  const loadTraces = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const res = await fetchRetrievalTraces();
      setTraces(res.traces);
      setSelected((prev) =>
        prev ? (res.traces.find((t) => t.timestamp === prev.timestamp) ?? null) : null,
      );
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : '加载失败');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadTraces();
  }, [loadTraces]);

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="flex items-center justify-between px-6 py-4 border-b border-[var(--color-border)]">
        <h2 className="text-lg font-semibold text-[var(--color-text-primary)]">检索轨迹</h2>
        <button
          onClick={loadTraces}
          disabled={loading}
          className="flex items-center gap-1 px-2 py-1 text-xs rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50"
        >
          <RefreshCw size={12} />
          刷新
        </button>
      </div>

      {/* Body: left list + right detail */}
      <div className="flex flex-1 overflow-hidden">
        {/* Left: trace list */}
        <div className="w-[40%] min-w-[260px] max-w-[360px] flex flex-col border-r border-[var(--color-border)]">
          <div className="flex-1 overflow-y-auto p-3">
            {error && (
              <div
                role="alert"
                className="flex items-center gap-2 p-3 rounded-lg bg-red-50 dark:bg-red-950 border border-red-200 dark:border-red-800 text-red-700 dark:text-red-300 text-sm"
              >
                <AlertCircle size={16} />
                <span>{error}</span>
              </div>
            )}

            {!error && loading && (
              <div className="flex items-center justify-center py-16">
                <Loader2 size={20} className="animate-spin text-[var(--color-text-tertiary)]" />
              </div>
            )}

            {!error && !loading && traces.length === 0 && (
              <div className="flex flex-col items-center justify-center py-16 text-[var(--color-text-tertiary)]">
                <Route size={40} className="mb-3 opacity-40" />
                <p className="text-sm">暂无检索轨迹</p>
              </div>
            )}

            {!error && !loading && traces.length > 0 && (
              <div className="space-y-1">
                {traces.map((trace) => (
                  <div
                    key={trace.timestamp}
                    onClick={() => setSelected(trace)}
                    className={`flex items-center justify-between px-3 py-2 rounded-md cursor-pointer transition-colors ${
                      selected?.timestamp === trace.timestamp
                        ? 'bg-blue-50 dark:bg-blue-900/30 border border-blue-200 dark:border-blue-800'
                        : 'border border-transparent hover:bg-[var(--color-bg-hover)]'
                    }`}
                  >
                    <div className="min-w-0 flex-1">
                      <p className="text-sm text-[var(--color-text-primary)] truncate">
                        {trace.query}
                      </p>
                      <p className="text-xs text-[var(--color-text-tertiary)] mt-0.5">
                        {formatDuration(trace.total_time_ms)} · {trace.total_tokens} Tokens
                      </p>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>

        {/* Right: trace detail */}
        <div className="flex-1 flex flex-col overflow-hidden">
          {!selected ? (
            <div className="flex-1 flex flex-col items-center justify-center text-[var(--color-text-tertiary)]">
              <Route size={48} className="mb-4 opacity-30" />
              <p className="text-sm">选择一条检索轨迹查看详情</p>
            </div>
          ) : (
            <div className="flex-1 overflow-y-auto p-6">
              {/* Query title */}
              <h3 className="text-base font-semibold text-[var(--color-text-primary)] mb-1">
                {selected.query}
              </h3>
              <p className="text-xs text-[var(--color-text-tertiary)] mb-5">
                {formatTime(selected.timestamp)}
              </p>

              {/* Steps timeline */}
              <h4 className="text-xs font-semibold text-[var(--color-text-tertiary)] uppercase tracking-wider mb-2">
                执行步骤
              </h4>
              <div className="mb-6">
                {selected.steps.map((step, i) => (
                  <div key={i} className="flex items-start gap-3">
                    <div className="flex flex-col items-center">
                      <div
                        className={`w-2 h-2 rounded-full mt-1.5 ${
                          step.score !== null ? 'bg-blue-500' : 'bg-[var(--color-text-tertiary)]'
                        }`}
                      />
                      {i < selected.steps.length - 1 && (
                        <div className="w-px flex-1 min-h-[16px] bg-[var(--color-border)]" />
                      )}
                    </div>
                    <div className="flex-1 pb-4 min-w-0">
                      <div className="flex items-center gap-2">
                        <span className="px-1.5 py-0.5 text-xs rounded bg-[var(--color-bg-secondary)] text-[var(--color-text-secondary)]">
                          {STEP_TYPE_LABELS[step.step_type]}
                        </span>
                        <span className="text-xs text-[var(--color-text-tertiary)]">
                          {formatTime(step.timestamp)}
                        </span>
                      </div>
                      <p
                        className="text-xs text-[var(--color-text-secondary)] mt-1 truncate"
                        title={step.target_uri.uri}
                      >
                        {step.target_uri.uri}
                      </p>
                      <div className="flex items-center gap-3 mt-0.5 text-xs text-[var(--color-text-tertiary)]">
                        <span>分数 {step.score !== null ? step.score.toFixed(3) : '—'}</span>
                        <span>{step.tokens_used} Tokens</span>
                      </div>
                    </div>
                  </div>
                ))}
              </div>

              {/* Results */}
              <h4 className="text-xs font-semibold text-[var(--color-text-tertiary)] uppercase tracking-wider mb-2">
                检索结果
              </h4>
              <div className="space-y-1 mb-6">
                {selected.results.length === 0 ? (
                  <p className="text-xs text-[var(--color-text-tertiary)] py-2">无检索结果</p>
                ) : (
                  selected.results.map((result) => (
                    <div
                      key={result.uri}
                      className="flex items-center gap-2 px-3 py-2 rounded-md bg-[var(--color-bg-secondary)] border border-[var(--color-border)]"
                    >
                      <FileText size={14} className="shrink-0 text-[var(--color-text-tertiary)]" />
                      <span
                        className="text-xs text-[var(--color-text-secondary)] truncate"
                        title={result.uri}
                      >
                        {result.uri}
                      </span>
                    </div>
                  ))
                )}
              </div>

              {/* Stats */}
              <h4 className="text-xs font-semibold text-[var(--color-text-tertiary)] uppercase tracking-wider mb-2">
                统计
              </h4>
              <div className="flex items-center gap-4 text-sm">
                <span className="flex items-center gap-1.5 text-[var(--color-text-secondary)]">
                  <Clock size={14} className="text-[var(--color-text-tertiary)]" />
                  总耗时 {formatDuration(selected.total_time_ms)}
                </span>
                <span className="flex items-center gap-1.5 text-[var(--color-text-secondary)]">
                  <Zap size={14} className="text-[var(--color-text-tertiary)]" />总 Token{' '}
                  {selected.total_tokens}
                </span>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
