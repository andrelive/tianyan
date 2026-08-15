import { useAppStore } from '@/lib/store';
import { Brain } from 'lucide-react';
import { cn } from '@/lib/utils';

/**
 * 会话级思考模式开关（参考 DSH / opencode 的会话时模型选择设计）：
 * 思考是否开启随对话选择，而不是全局设置；仅对支持思考的模型生效，
 * 不支持时模型侧会忽略该参数。
 */
export default function ThinkingToggle() {
  const thinkingOn = useAppStore((s) => s.thinkingOn);
  const setThinkingOn = useAppStore((s) => s.setThinkingOn);

  return (
    <button
      type="button"
      onClick={() => setThinkingOn(!thinkingOn)}
      role="switch"
      aria-checked={thinkingOn}
      aria-label="思考模式"
      title={thinkingOn ? '思考模式：开启（仅对支持思考的模型生效）' : '思考模式：关闭'}
      className={cn(
        'flex items-center gap-1.5 px-3 py-1.5 text-sm rounded-lg border transition-colors',
        thinkingOn
          ? 'border-blue-500 bg-blue-50 text-blue-700 dark:bg-blue-900/30 dark:text-blue-300'
          : 'border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text-primary)]',
      )}
    >
      <Brain size={14} className={cn(thinkingOn ? '' : 'opacity-60')} />
      <span>思考</span>
    </button>
  );
}
