import type { StreamUsage } from '@/lib/types';

/** token 数显示：≥1k 用 k 记（1k 小数 1 位，≥10k 取整）。 */
function formatTokens(n: number): string {
  if (n >= 1000) {
    const v = n >= 10000 ? Math.round(n / 1000) : Math.round((n / 1000) * 10) / 10;
    return `${v}k`;
  }
  return String(n);
}

/**
 * 上下文占用 / 缓存命中指示（对齐 DSH ContextMeter 展示语义）：
 * - 上下文占用 = 本轮请求 prompt_tokens（含缓存命中部分），百分比 = prompt / 模型上下文窗口；
 * - 缓存命中 = cache_read tokens 及其占 prompt 的比例（提供商返回缓存明细时才有意义）。
 */
export default function UsageMeter({ usage }: { usage: StreamUsage | null }) {
  if (!usage || usage.prompt_tokens <= 0) return null;
  const { prompt_tokens, cache_read, context_window } = usage;
  const pct =
    context_window > 0 ? Math.min(100, Math.round((prompt_tokens / context_window) * 100)) : 0;
  const cachePct =
    prompt_tokens > 0 ? Math.min(100, Math.round((cache_read / prompt_tokens) * 100)) : 0;
  return (
    <div
      className="px-4 pb-1.5 select-none"
      title={`本轮请求上下文占用 ${prompt_tokens} token（窗口 ${context_window}），缓存命中 ${cache_read} token（提供商返回明细时有效）`}
    >
      <div className="flex items-center justify-between text-[11px] text-[var(--color-text-tertiary)]">
        <span>
          上下文 
          <span className="text-[var(--color-text-secondary)]">
            {formatTokens(prompt_tokens)} / {formatTokens(context_window)}
          </span>
          <span className="ml-1.5">{pct}%</span>
        </span>
        <span>
          缓存命中 
          <span className="text-[var(--color-text-secondary)]">{formatTokens(cache_read)}</span>
          <span className="ml-1.5">{cachePct}%</span>
        </span>
      </div>
      <div className="mt-1 h-1 rounded-full bg-[var(--color-bg-hover)] overflow-hidden">
        <div
          className="h-full rounded-full bg-[var(--color-accent)] transition-[width] duration-300"
          style={{ width: `${pct}%` }}
        />
      </div>
    </div>
  );
}