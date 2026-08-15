import { useState, useRef, useEffect } from 'react';
import { useAppStore } from '@/lib/store';
import { Brain, ChevronDown, Check } from 'lucide-react';
import { cn } from '@/lib/utils';

/** 内置"关闭"档位（不附加思考参数；不属于模型声明，恒提供）。 */
const OFF_EFFORT = 'off';

/**
 * 会话级思考强度选择：档位集来自**当前模型自己声明的值**（后端 /config/models
 * 返回 reasoning_efforts，显式配置 > 内置模型表），**原样展示不做本地翻译**
 * （厂商档位可能为 low/high/max 等任意值）；模型不支持思考时隐藏。
 */
export default function ThinkingSelect() {
  const thinkingEffort = useAppStore((s) => s.thinkingEffort);
  const setThinkingEffort = useAppStore((s) => s.setThinkingEffort);
  const selectedModel = useAppStore((s) => s.selectedModel);
  const chatModels = useAppStore((s) => s.chatModels);
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  // 当前模型声明的档位值（每个模型自己的；缺省 = 不支持思考 → 隐藏）
  const declared: string[] | undefined = chatModels.find(
    (m) => m.name === selectedModel,
  )?.reasoning_efforts ?? undefined;
  const supportsThinking =
    !!declared && declared.length > 0 && !(declared.length === 1 && declared[0] === OFF_EFFORT);

  // 选项：恒含"关闭"，外加模型声明的档位值（去重、原样显示）
  const options: string[] = supportsThinking
    ? [OFF_EFFORT, ...declared!.filter((v) => v !== OFF_EFFORT)]
    : [];

  // 模型切换后若已选档位不在新模型档位集内 → 重置为关闭
  useEffect(() => {
    if (supportsThinking && !options.includes(thinkingEffort)) {
      setThinkingEffort(OFF_EFFORT);
    }
  }, [declared, thinkingEffort, setThinkingEffort]); // eslint-disable-line react-hooks/exhaustive-deps -- options 由 declared 派生

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

  if (!supportsThinking) return null;

  const current = thinkingEffort;
  const active = current !== OFF_EFFORT;

  return (
    <div ref={ref} className="relative">
      <button
        type="button"
        onClick={() => setOpen(!open)}
        aria-label="思考强度"
        aria-expanded={open}
        aria-haspopup="listbox"
        title={'思考强度：' + current + '（当前模型声明的档位）'}
        className={cn(
          'flex items-center gap-1.5 px-3 py-1.5 text-sm rounded-lg border transition-colors',
          active
            ? 'border-blue-500 bg-blue-50 text-blue-700 dark:bg-blue-900/30 dark:text-blue-300'
            : 'border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text-primary)]',
        )}
      >
        <Brain size={14} className={cn(active ? '' : 'opacity-60')} />
        <span>{'思考 ' + current}</span>
        <ChevronDown className={cn('w-3.5 h-3.5 transition-transform', open && 'rotate-180')} />
      </button>

      {open && (
        <div
          role="listbox"
          aria-label="思考强度列表"
          className="absolute right-0 top-full mt-1 w-48 bg-[var(--color-bg-primary)] border border-[var(--color-border)] rounded-lg shadow-lg z-50 py-1"
        >
          {options.map((opt) => (
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
              <span>{opt === OFF_EFFORT ? '关闭' : opt}</span>
              {opt === current && <Check size={14} className="shrink-0" />}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
