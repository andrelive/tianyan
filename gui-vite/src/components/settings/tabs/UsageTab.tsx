import { useEffect, useState } from 'react';
import { BarChart3, Loader2, Search } from 'lucide-react';
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
  start_ts: number | null;
  end_ts: number | null;
  total: UsageStat | null;
  grouped: UsageStat[];
}

function fmt(n: number): string {
  return n.toLocaleString('en-US');
}

function pct(n: number): string {
  return Math.round(n * 100) + '%';
}

/** 快捷时段定义：1d / 3d / 7d / 30d / 全部。 */
const QUICK_RANGES: { label: string; days: number }[] = [
  { label: '1d', days: 1 },
  { label: '3d', days: 3 },
  { label: '7d', days: 7 },
  { label: '30d', days: 30 },
  { label: '全部', days: 0 },
];

type ActiveQuery =
  | { kind: 'quick'; days: number }
  | { kind: 'custom'; start: string; end: string };

/** 统计面板：LLM token 消耗（未命中输入 / 缓存命中输入 / 输出 / 命中率）。
 *  支持快捷时段与自定义起止日期查询。 */
export default function UsageTab() {
  // 自定义日期输入（仅编辑态；生效查询在 activeQuery）
  const [startDate, setStartDate] = useState('');
  const [endDate, setEndDate] = useState('');
  // 生效查询：点击快捷按钮或「查询」后固化
  const [activeQuery, setActiveQuery] = useState<ActiveQuery>({ kind: 'quick', days: 7 });
  const [data, setData] = useState<UsageStatsResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // 当前查询的 API 参数串
  const queryRange =
    activeQuery.kind === 'quick'
      ? 'days=' + activeQuery.days
      : 'start_ts=' +
        Math.floor(new Date(activeQuery.start + 'T00:00:00').getTime() / 1000) +
        '&end_ts=' +
        Math.floor(new Date(activeQuery.end + 'T23:59:59').getTime() / 1000);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    apiGet<UsageStatsResponse>('/usage/stats?' + queryRange)
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
  }, [queryRange]);

  const pickQuick = (d: number) => {
    setActiveQuery({ kind: 'quick', days: d });
  };

  const applyCustom = () => {
    if (!startDate || !endDate) {
      setError('请选择起止日期');
      return;
    }
    if (startDate > endDate) {
      setError('起始日期不能晚于结束日期');
      return;
    }
    setError(null);
    setActiveQuery({ kind: 'custom', start: startDate, end: endDate });
  };

  const rangeLabel =
    activeQuery.kind === 'quick'
      ? activeQuery.days === 0
        ? '全部时间'
        : '最近 ' + activeQuery.days + ' 天'
      : activeQuery.start + ' ~ ' + activeQuery.end;

  const total = data?.total;
  const grouped = data?.grouped ?? [];

  return (
    <div>
      <SectionTitle title="Token 用量统计" />
      <p className="text-xs text-[var(--color-text-tertiary)] mb-3">
        覆盖聊天、子代理委托、自演化任务等所有 LLM 调用；缓存命中率 = 命中输入 / 总输入。
      </p>

      {/* 查询区：快捷时段 + 自定义日期范围 */}
      <div className="flex flex-wrap items-center gap-2 mb-3">
        {QUICK_RANGES.map((r) => (
          <button
            key={r.label}
            type="button"
            onClick={() => pickQuick(r.days)}
            className={
              'px-2.5 py-1 text-xs rounded-md border transition-colors ' +
              (activeQuery.kind === 'quick' && activeQuery.days === r.days
                ? 'border-accent bg-accent/10 text-[var(--color-text-primary)]'
                : 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]')
            }
          >
            {r.label}
          </button>
        ))}
        <span className="mx-1 opacity-40 text-xs">|</span>
        <input
          type="date"
          value={startDate}
          onChange={(e) => setStartDate(e.target.value)}
          aria-label="起始日期"
          className="px-2 py-1 text-xs rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
        />
        <span className="text-xs text-[var(--color-text-tertiary)]">~</span>
        <input
          type="date"
          value={endDate}
          onChange={(e) => setEndDate(e.target.value)}
          aria-label="结束日期"
          className="px-2 py-1 text-xs rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
        />
        <button
          type="button"
          onClick={applyCustom}
          className="flex items-center gap-1 px-2.5 py-1 text-xs rounded-md bg-accent text-white hover:bg-accent-hover transition-colors"
        >
          <Search size={12} />
          查询
        </button>
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
          <div className="text-[11px] text-[var(--color-text-tertiary)] mb-2">
            {rangeLabel} · {total.calls} 次 LLM 调用
          </div>
          {/* 总计卡片 */}
          <div className="grid grid-cols-2 md:grid-cols-4 gap-2 mb-4">
            <MetricCard label="未命中输入" value={fmt(total.uncached_input)} />
            <MetricCard label="缓存命中输入" value={fmt(total.cached_input)} />
            <MetricCard label="输出" value={fmt(total.completion_tokens)} />
            <MetricCard label="缓存命中率" value={pct(total.cache_hit_rate)} />
          </div>
          <div className="flex items-center gap-2 text-[11px] text-[var(--color-text-tertiary)] mb-3">
            <BarChart3 className="w-3.5 h-3.5" />
            总 {fmt(total.total_tokens)} token
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
