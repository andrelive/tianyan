import { useState, useRef, useEffect } from 'react';
import { useAppStore } from '@/lib/store';
import { Brain, ChevronDown, Check } from 'lucide-react';
import { cn } from '@/lib/utils';
import type { ThinkingEffort } from '@/lib/types';

/** 思考强度选项（与后端 ThinkingEffort 对齐；off 不附加思考参数）。 */
const EFFORT_OPTIONS: { value: ThinkingEffort; label: string; hint: string }[] = [
  { value: 'off', label: '关闭', hint: '不附加思考参数，使用模型默认行为' },
  { value: 'low', label: '低', hint: '低强度思考（reasoning_effort=low）' },
  { value: 'medium', label: '中', hint: '中强度思考（reasoning_effort=medium）' },
  { value: 'high', label: '高', hint: '高强度思考（reasoning_effort=high）' },
];

/**
 * 会话级思考强度选择（参考 DSH ModelSelect 的 Effort 选择、opencode 的
 * variants 切换）：思考强度随对话选择，而不是全局设置；仅对支持思考的
 * 模型生效，不支持的模型会忽略对应参数。
 */
export default function ThinkingSelect() {
  const thinkingEffort = useAppStore((s) => s.thinkingEffort);
  const setThinkingEffort = useAppStore((s) => s.setThinkingEffort);
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
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
  }, []);

  const current = EFFORT_OPTIONS.find((o) => o.value === thinkingEffort) ?? EFFORT_OPTIONS[0];
  const active = current.value !== 'off';

  return (
    <div ref={ref} className="relative">
      <button
        type="button"
        onClick={() => setOpen(!open)}
        aria-label="思考强度"
        aria-expanded={open}
        aria-haspopup="listbox"
        title={`思考强度：${current.label}（仅对支持思考的模型生效）`}
        className={cn(
          'flex items-center gap-1.5 px-3 py-1.5 text-sm rounded-lg border transition-colors',
          active
            ? 'border-blue-500 bg-blue-50 text-blue-700 dark:bg-blue-900/30 dark:text-blue-300'
            : 'border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text-primary)]',
        )}
      >
        <Brain size={14} className={cn(active ? '' : 'opacity-60')} />
        <span>思考 {current.label}</span>
        <ChevronDown className={cn('w-3.5 h-3.5 transition-transform', open && 'rotate-180')} />
      </button>

      {open && (
        <div
          role="listbox"
          aria-label="思考强度列表"
          className="absolute right-0 top-full mt-1 w-56 bg-[var(--color-bg-primary)] border border-[var(--color-border)] rounded-lg shadow-lg z-50 py-1"
        >
          {EFFORT_OPTIONS.map((opt) => (
            <button
              key={opt.value}
              role="option"
              aria-selected={opt.value === thinkingEffort}
              onClick={() => {
                setThinkingEffort(opt.value);
                setOpen(false);
              }}
              className={cn(
                'w-full text-left px-3 py-2 text-sm hover:bg-[var(--color-bg-hover)] transition-colors flex items-center justify-between gap-2',
                opt.value === thinkingEffort
                  ? 'text-blue-600 dark:text-blue-400 font-medium'
                  : 'text-[var(--color-text-primary)]',
              )}
            >
              <span className="flex flex-col items-start">
                <span>{opt.label}</span>
                <span className="text-xs text-[var(--color-text-tertiary)] font-normal">
                  {opt.hint}
                </span>
              </span>
              {opt.value === thinkingEffort && <Check size={14} className="shrink-0" />}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
