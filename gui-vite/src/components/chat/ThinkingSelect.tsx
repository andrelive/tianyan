import { useState, useRef, useEffect } from 'react';
import { useAppStore } from '@/lib/store';
import { Brain, ChevronDown, Check } from 'lucide-react';
import { cn } from '@/lib/utils';
import type { ThinkingEffort } from '@/lib/types';

/** 档位显示名（未知档位回退显示原 id）。 */
const EFFORT_LABELS: Record<string, string> = {
  off: '关闭',
  low: '低',
  medium: '中',
  high: '高',
};

/** 档位说明。 */
const EFFORT_HINTS: Record<string, string> = {
  off: '不附加思考参数，使用模型默认行为',
  low: '低强度思考',
  medium: '中强度思考',
  high: '高强度思考',
};

/**
 * 会话级思考强度选择（参考 DSH ModelSelect 的 Effort 选择、opencode 的
 * variants 切换）：档位集来自**当前模型自己的声明**（后端 /config/models
 * 返回 reasoning_efforts，显式配置 > 内置模型表），模型不支持思考时隐藏。
 */
export default function ThinkingSelect() {
  const thinkingEffort = useAppStore((s) => s.thinkingEffort);
  const setThinkingEffort = useAppStore((s) => s.setThinkingEffort);
  const selectedModel = useAppStore((s) => s.selectedModel);
  const chatModels = useAppStore((s) => s.chatModels);
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  // 当前模型声明的档位（每个模型自己的；缺省 = 不支持思考 → 隐藏）
  const efforts: ThinkingEffort[] | undefined = chatModels.find(
    (m) => m.name === selectedModel,
  )?.reasoning_efforts ?? undefined;
  const supportsThinking = !!efforts && efforts.length > 0 && !(efforts.length === 1 && efforts[0] === 'off');

  // 模型切换后若已选档位不在新模型档位集内 → 重置为 off
  useEffect(() => {
    if (efforts && !efforts.includes(thinkingEffort)) {
      setThinkingEffort('off');
    }
  }, [efforts, thinkingEffort, setThinkingEffort]);

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

  if (!supportsThinking || !efforts) return null;

  const current = thinkingEffort;
  const active = current !== 'off';

  return (
    <div ref={ref} className="relative">
      <button
        type="button"
        onClick={() => setOpen(!open)}
        aria-label="思考强度"
        aria-expanded={open}
        aria-haspopup="listbox"
        title={'思考强度：' + (EFFORT_LABELS[current] ?? current) + '（当前模型支持思考）'}
        className={cn(
          'flex items-center gap-1.5 px-3 py-1.5 text-sm rounded-lg border transition-colors',
          active
            ? 'border-blue-500 bg-blue-50 text-blue-700 dark:bg-blue-900/30 dark:text-blue-300'
            : 'border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text-primary)]',
        )}
      >
        <Brain size={14} className={cn(active ? '' : 'opacity-60')} />
        <span>{'思考 ' + (EFFORT_LABELS[current] ?? current)}</span>
        <ChevronDown className={cn('w-3.5 h-3.5 transition-transform', open && 'rotate-180')} />
      </button>

      {open && (
        <div
          role="listbox"
          aria-label="思考强度列表"
          className="absolute right-0 top-full mt-1 w-56 bg-[var(--color-bg-primary)] border border-[var(--color-border)] rounded-lg shadow-lg z-50 py-1"
        >
          {efforts.map((opt) => (
            <button
              key={opt}
              role="option"
              aria-selected={opt === current}
              onClick={() => {
                setThinkingEffort(opt);
                setOpen(false);
              }}
              className={cn(
                'w-full text-left px-3 py-2 text-sm hover:bg-[var(--color-bg-hover)] transition-colors flex items-center justify-between gap-2',
                opt === current
                  ? 'text-blue-600 dark:text-blue-400 font-medium'
                  : 'text-[var(--color-text-primary)]',
              )}
            >
              <span className="flex flex-col items-start">
                <span>{EFFORT_LABELS[opt] ?? opt}</span>
                <span className="text-xs text-[var(--color-text-tertiary)] font-normal">
                  {EFFORT_HINTS[opt] ?? '思考强度档位'}
                </span>
              </span>
              {opt === current && <Check size={14} className="shrink-0" />}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
