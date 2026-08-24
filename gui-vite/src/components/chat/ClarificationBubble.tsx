import { useState, useCallback } from 'react';
import { HelpCircle, Loader2, Send } from 'lucide-react';
import { cn } from '@/lib/utils';

interface Props {
  question: string;
  /** 候选选项（label 列表；空数组 = 纯文本输入） */
  options: string[];
  submitting: boolean;
  onSubmit: (answer: string) => void;
}

/**
 * 追问接管组件（composer takeover，对齐 DSH ask_user_question）：
 * Agent 需要确认时，输入框区域被问题表单接管——问题文本 + N 个选项
 * （单选）+ 1 个自定义输入；提交后通过 /chat/clarify/stream 继续处理
 * （流式），流结束后恢复普通输入框。
 */
export default function ClarificationBubble({ question, options, submitting, onSubmit }: Props) {
  const [selected, setSelected] = useState<string | null>(null);
  const [custom, setCustom] = useState('');

  const hasOptions = options.length > 0;
  const answer = selected ?? custom.trim();
  const canSubmit = answer.length > 0 && !submitting;

  const handleSubmit = useCallback(() => {
    if (!canSubmit) return;
    onSubmit(answer);
    setSelected(null);
    setCustom('');
  }, [canSubmit, answer, onSubmit]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        handleSubmit();
      }
    },
    [handleSubmit],
  );

  return (
    <div className="w-full px-4 py-3 border border-[var(--color-border)] bg-[var(--color-bg-secondary)] text-[var(--color-text-primary)]">
      {/* 标签：AI 需要确认（输入框接管模式：普通输入区隐藏，此处回答） */}
      <div className="flex items-center gap-1.5 mb-1.5">
        <HelpCircle className="w-4 h-4 text-amber-600 dark:text-amber-500" aria-hidden="true" />
        <span className="text-xs font-medium text-amber-700 dark:text-amber-500">AI 需要确认</span>
      </div>

      {/* 追问问题 */}
      <p className="text-sm leading-relaxed break-words whitespace-pre-wrap mb-2">{question}</p>

      {/* 选项（单选）+ 自定义输入：对齐 DSH 的 N 选项 + 1 自定义形态 */}
      {hasOptions && (
        <div role="radiogroup" aria-label="回答选项" className="flex flex-wrap gap-2 mb-2">
          {options.map((label) => {
            const active = selected === label;
            return (
              <button
                key={label}
                type="button"
                role="radio"
                aria-checked={active}
                onClick={() => {
                  setSelected(active ? null : label);
                  setCustom('');
                }}
                disabled={submitting}
                className={cn(
                  'px-3 py-1.5 rounded-full text-sm border transition-colors',
                  active
                    ? 'border-amber-500 bg-amber-500/10 text-amber-700 dark:text-amber-500'
                    : 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:border-amber-500/50',
                  'disabled:opacity-50 disabled:cursor-not-allowed',
                )}
              >
                {label}
              </button>
            );
          })}
        </div>
      )}

      {/* 回答输入（自定义；有选项时作为补充输入） */}
      <div className="flex items-end gap-2">
        <textarea
          value={custom}
          onChange={(e) => {
            setCustom(e.target.value);
            if (e.target.value.trim() !== '') setSelected(null);
          }}
          onKeyDown={handleKeyDown}
          rows={2}
          disabled={submitting}
          placeholder={
            hasOptions
              ? '或输入自定义回答... (Shift+Enter 换行)'
              : '输入你的回答... (Shift+Enter 换行)'
          }
          aria-label="输入对追问的回答"
          className={cn(
            'flex-1 resize-none rounded-xl border border-[var(--color-border)]',
            'bg-[var(--color-bg-primary)] px-3 py-2',
            'text-sm text-[var(--color-text-primary)]',
            'placeholder:text-[var(--color-text-tertiary)]',
            'focus:outline-none focus:ring-2 focus:ring-amber-500/30 focus:border-amber-500',
            'disabled:opacity-50 disabled:cursor-not-allowed',
            'transition-colors',
          )}
        />
        <button
          onClick={handleSubmit}
          disabled={!canSubmit}
          aria-label="提交回答"
          className={cn(
            'flex items-center gap-1.5 px-3 py-2 rounded-xl text-sm font-medium transition-colors shrink-0',
            canSubmit
              ? 'bg-amber-600 hover:bg-amber-700 text-white'
              : 'bg-[var(--color-bg-tertiary)] text-[var(--color-text-tertiary)] cursor-not-allowed',
          )}
        >
          {submitting ? <Loader2 className="w-4 h-4 animate-spin" /> : <Send className="w-4 h-4" />}
          {submitting ? '发送中...' : '发送'}
        </button>
      </div>
    </div>
  );
}
