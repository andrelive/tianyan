import { useEffect, useState } from 'react';
import { BarChart3, Loader2 } from 'lucide-react';
import { apiGet } from '@/lib/api-client';
import { SectionTitle } from './shared';

/** 单条用量统计（总计或按 provider/model 分组）。 */
interface UsageStat {
  group: string;
  calls: number;
  uncached_input: number;
  cached_input: number;
  completion_tokens: number;
  total_tokens: number;
  cache_hit_rate: number;
}

interface UsageStatsResponse {
  days: number;
  total: UsageStat | null;
  grouped: UsageStat[];
}

function fmt(n: number): string {
  return n.toLocaleString('en-US');
}

function pct(n: number): string {
  return Math.round(n * 100) + '%';
}

/** 统计面板：LLM token 消耗（缓存未命中输入 / 缓存命中输入 / 输出 / 命中率）。 */
export default function UsageTab() {
  const [days, setDays] = useState<number>(7);
  const [data, setData] = useState<UsageStatsResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    apiGet<UsageStatsResponse>(`/usage/stats?days=${days}`)
      .then((resp) => {
        if (!cancelled) setData(resp);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof Error ? e.message : '加载失败');
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [days]);

  const total = data?.total;
  const grouped = data?.grouped ?? [];

  return (
    <div>
      <SectionTitle title="Token 用量统计" />
      <p className="text-xs text-[var(--color-text-tertiary)] mb-3">
        覆盖聊天、子代理委托、自演化任务等所有 LLM 调用；缓存命中率 = 命中输入 / 总输入。
      </p>

      {/* 时段选择 */}
      <div className="flex items-center gap-2 mb-4">
        <span className="text-xs text-[var(--color-text-secondary)]">时段</span>
        {[7, 30, 0].map((d) => (
          <button
            key={d}
            type="button"
            onClick={() => setDays(d)}
            className={
              'px-2.5 py-1 text-xs rounded-md border transition-colors ' +
              (days === d
                ? 'border-accent bg-accent/10 text-[var(--color-text-primary)]'
                : 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]')
            }
          >
            {d === 0 ? '全部' : d + ' 天'}
          </button>
        ))}
      </div>

      {loading && (
        <div className="flex items-center gap-2 text-xs text-[var(--color-text-tertiary)] py-6">
          <Loader2 className="w-3.5 h-3.5 animate-spin" />
          加载中…
        </div>
      )}
      {error && (
        <p className="text-xs text-red-500 py-3" role="alert">
          {error}
        </p>
      )}

      {!loading && !error && total && (
        <>
          {/* 总计卡片 */}
          <div className="grid grid-cols-2 md:grid-cols-4 gap-2 mb-4">
            <MetricCard label="未命中输入" value={fmt(total.uncached_input)} />
            <MetricCard label="缓存命中输入" value={fmt(total.cached_input)} />
            <MetricCard label="输出" value={fmt(total.completion_tokens)} />
            <MetricCard label="缓存命中率" value={pct(total.cache_hit_rate)} />
          </div>
          <div className="flex items-center gap-2 text-[11px] text-[var(--color-text-tertiary)] mb-3">
            <BarChart3 className="w-3.5 h-3.5" />
            {total.calls} 次 LLM 调用 · 总 {fmt(total.total_tokens)} token
          </div>

          {/* 按模型分组 */}
          {grouped.length > 0 && (
            <table className="w-full text-xs border-collapse">
              <thead>
                <tr className="text-left text-[var(--color-text-tertiary)] border-b border-[var(--color-border)]">
                  <th className="py-1.5 pr-2 font-medium">模型</th>
                  <th className="py-1.5 pr-2 font-medium text-right">调用</th>
                  <th className="py-1.5 pr-2 font-medium text-right">未命中输入</th>
                  <th className="py-1.5 pr-2 font-medium text-right">命中输入</th>
                  <th className="py-1.5 pr-2 font-medium text-right">输出</th>
                  <th className="py-1.5 font-medium text-right">命中率</th>
                </tr>
              </thead>
              <tbody>
                {grouped.map((s) => (
                  <tr
                    key={s.group}
                    className="border-b border-[var(--color-border)]/50 text-[var(--color-text-primary)]"
                  >
                    <td className="py-1.5 pr-2 font-mono text-xs">{s.group}</td>
                    <td className="py-1.5 pr-2 text-right font-mono text-xs">{s.calls}</td>
                    <td className="py-1.5 pr-2 text-right font-mono text-xs">{fmt(s.uncached_input)}</td>
                    <td className="py-1.5 pr-2 text-right font-mono text-xs">{fmt(s.cached_input)}</td>
                    <td className="py-1.5 pr-2 text-right font-mono text-xs">{fmt(s.completion_tokens)}</td>
                    <td className="py-1.5 text-right font-mono text-xs">{pct(s.cache_hit_rate)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </>
      )}

      {!loading && !error && !total && (
        <p className="text-xs text-[var(--color-text-tertiary)] py-4">
          所选时段内暂无 LLM 调用记录。
        </p>
      )}
    </div>
  );
}

function MetricCard({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-secondary)] px-3 py-2">
      <div className="text-[11px] text-[var(--color-text-tertiary)]">{label}</div>
      <div className="font-mono text-sm text-[var(--color-text-primary)]">{value}</div>
    </div>
  );
}
