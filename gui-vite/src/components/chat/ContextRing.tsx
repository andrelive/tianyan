import { useState, useEffect, useRef } from 'react';
import { Loader2, Minimize2 } from 'lucide-react';
import type { StreamUsage } from '@/lib/types';
import { formatNumber } from '@/lib/utils';

/** 圆环半径（viewBox 32×32）。 */
const R = 13;
const CIRCUMFERENCE = 2 * Math.PI * R;

/** 上下文占用阈值配色（对齐 DSH ContextMeter：临界 80% 红 / 高 50% 琥珀 / 正常绿） */
function ringColor(pct: number): string {
  // 引用主题变量（--color-error/warning/success），不再硬编码 hex
  if (pct >= 80) return 'var(--color-error)';
  if (pct >= 50) return 'var(--color-warning)';
  return 'var(--color-success)';
}

/**
 * 上下文占用圆环（DSH ContextMeter 风格）：环径 = 占用百分比，
 * 中心数字 = 百分比；点击展开详情面板（占用 / 缓存命中 / 输出）
 * 并提供「压缩会话」快捷操作。
 */
export default function ContextRing({
  usage,
  onCompress,
  compressing = false,
}: {
  usage: StreamUsage | null;
  /** 压缩当前会话（由父组件提供，卧交给元组件） */
  onCompress: () => void;
  /** 压缩进行中（禁用按钮 + 显示 loading，防重复点击） */
  compressing?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  const pct =
    usage && usage.context_window > 0
      ? Math.min(100, Math.max(0, (usage.prompt_tokens / usage.context_window) * 100))
      : 0;
  const cachePct =
    usage && usage.prompt_tokens > 0
      ? Math.min(100, Math.max(0, (usage.cache_read / usage.prompt_tokens) * 100))
      : 0;

  // 点击外部 / Esc 关闭详情面板
  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const escapeHandler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false);
    };
    document.addEventListener('mousedown', handler);
    document.addEventListener('keydown', escapeHandler);
    return () => {
      document.removeEventListener('mousedown', handler);
      document.removeEventListener('keydown', escapeHandler);
    };
  }, [open]);

  const color = usage ? ringColor(pct) : 'var(--color-border)';
  const offset = CIRCUMFERENCE * (1 - pct / 100);
  const centerText = usage ? Math.round(pct) + '%' : '–';

  return (
    <div ref={ref} className="relative shrink-0">
      <button
        type="button"
        onClick={() => setOpen(!open)}
        aria-label="上下文占用"
        aria-expanded={open}
        title="上下文占用（点击查看详情 / 压缩）"
        className="block p-1 rounded-lg hover:bg-[var(--color-bg-hover)] transition-colors"
      >
        <svg viewBox="0 0 32 32" className="w-8 h-8 -rotate-90">
          <circle
            cx="16"
            cy="16"
            r={R}
            fill="none"
            stroke="var(--color-border)"
            strokeWidth="3.5"
          />
          <circle
            cx="16"
            cy="16"
            r={R}
            fill="none"
            stroke={color}
            strokeWidth="3.5"
            strokeLinecap="round"
            strokeDasharray={CIRCUMFERENCE}
            strokeDashoffset={offset}
            style={{ transition: 'stroke-dashoffset 0.5s ease, stroke 0.3s ease' }}
          />
        </svg>
        <span className="absolute inset-0 flex items-center justify-center text-[9px] font-medium text-[var(--color-text-secondary)]">
          {centerText}
        </span>
      </button>

      {open && usage && (
        <div
          role="dialog"
          aria-label="上下文占用详情"
          className="absolute right-0 bottom-full mb-2 w-60 bg-[var(--color-bg-primary)] border border-[var(--color-border)] rounded-lg shadow-lg z-50 p-3 text-xs"
        >
          <div className="flex items-center justify-between mb-1">
            <span className="text-[var(--color-text-tertiary)]">上下文占用</span>
            <span className="font-mono text-[var(--color-text-primary)]">
              {formatNumber(usage.prompt_tokens)} / {formatNumber(usage.context_window)} (
              {Math.round(pct)}%)
            </span>
          </div>
          <div className="flex items-center justify-between mb-1">
            <span className="text-[var(--color-text-tertiary)]">缓存命中</span>
            <span className="font-mono text-[var(--color-text-primary)]">
              {usage.cache_read > 0
                ? formatNumber(usage.cache_read) + ' token (' + Math.round(cachePct) + '%)'
                : '0（提供商未返回明细）'}
            </span>
          </div>
          <div className="flex items-center justify-between mb-2">
            <span className="text-[var(--color-text-tertiary)]">输出</span>
            <span className="font-mono text-[var(--color-text-primary)]">
              {formatNumber(usage.completion_tokens)} token
            </span>
          </div>
          {/* 口径说明：圆环 = 最近一轮请求（上下文占用语义）；底部小字 = 会话累计 */}
          <div className="pt-1 mb-2 border-t border-[var(--color-border)] text-[10px] text-[var(--color-text-tertiary)]">
            以上为最近一轮请求 · 输入框下方小字为会话累计
          </div>
          <button
            type="button"
            onClick={() => {
              // 压缩期间保持面板打开：按钮就地变为「压缩中...」（spinner +
              // disabled），用户能看到明确的进行中反馈；完成后由父组件
              // toast 提示，面板仍可点击外部/Esc 关闭。
              onCompress();
            }}
            disabled={compressing}
            className="w-full flex items-center justify-center gap-1.5 py-1.5 rounded-lg border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text-primary)] transition-colors disabled:opacity-50 disabled:cursor-not-allowed disabled:hover:bg-transparent disabled:hover:text-[var(--color-text-secondary)]"
          >
            {compressing ? (
              <>
                <Loader2 className="w-3.5 h-3.5 animate-spin" />
                压缩中...
              </>
            ) : (
              <>
                <Minimize2 className="w-3.5 h-3.5" />
                压缩会话（当前占用 {Math.round(pct)}%）
              </>
            )}
          </button>
        </div>
      )}
    </div>
  );
}
