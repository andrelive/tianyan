import { useState, useCallback } from 'react';
import { HelpCircle, Loader2, Send } from 'lucide-react';
import { cn } from '@/lib/utils';

interface Props {
  question: string;
  submitting: boolean;
  onSubmit: (answer: string) => void;
}

/**
 * 追问气泡：Agent 需要用户确认时展示问题与回答输入框，
 * 提交后通过 /chat/clarify/stream 继续处理（流式）。
 */
export default function ClarificationBubble({ question, submitting, onSubmit }: Props) {
  const [answer, setAnswer] = useState('');

  const canSubmit = answer.trim().length > 0 && !submitting;

  const handleSubmit = useCallback(() => {
    if (!canSubmit) return;
    onSubmit(answer.trim());
    setAnswer('');
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
    <div className="max-w-[80%] rounded-2xl rounded-br-sm px-4 py-3 border border-[var(--color-border)] bg-[var(--color-bg-secondary)] text-[var(--color-text-primary)]">
      {/* 标签：AI 需要确认 */}
      <div className="flex items-center gap-1.5 mb-1.5">
        <HelpCircle className="w-4 h-4 text-amber-600 dark:text-amber-500" aria-hidden="true" />
        <span className="text-xs font-medium text-amber-700 dark:text-amber-500">AI 需要确认</span>
      </div>

      {/* 追问问题 */}
      <p className="text-sm leading-relaxed break-words whitespace-pre-wrap">{question}</p>

      {/* 回答输入 */}
      <div className="mt-2 flex items-end gap-2">
        <textarea
          value={answer}
          onChange={(e) => setAnswer(e.target.value)}
          onKeyDown={handleKeyDown}
          rows={2}
          disabled={submitting}
          placeholder="输入你的回答... (Shift+Enter 换行)"
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
