import { useState, useCallback } from 'react';
import { HelpCircle, Loader2, Send } from 'lucide-react';
import { cn } from '@/lib/utils';

interface QuestionItem {
  question: string;
  options: { label: string; description?: string | null }[];
}

interface Props {
  /** 多问题列表（每个问题一个 tab；空 = 纯文本输入） */
  questions: QuestionItem[];
  submitting: boolean;
  /** 提交回答（JSON 字符串：{answers: [{question, answer}], extra}） */
  onSubmit: (answer: string) => void;
}

/**
 * 追问接管组件（composer takeover，对齐 DSH ask_user_question）：
 * Agent 需要确认时，输入框区域被问题表单接管——多问题分步 tab（每个
 * 问题一个 tab）+ 补充信息 tab；每个问题渲染为可点选项行（序号 +
 * label + description）+ 自定义输入。提交后通过 /chat/clarify/stream
 * 继续处理（流式），流结束后恢复普通输入框。
 */
export default function ClarificationBubble({ questions, submitting, onSubmit }: Props) {
  const [tab, setTab] = useState(0);
  const [selected, setSelected] = useState<(string | null)[]>(() => questions.map(() => null));
  const [customs, setCustoms] = useState<string[]>(() => questions.map(() => ''));
  const [extra, setExtra] = useState('');

  const EXTRA_TAB = questions.length; // 补充信息 tab 恒为最后一个
  const isExtra = tab === EXTRA_TAB;
  const question = questions[tab];

  // 稳定化：answerOf 被 handleSubmit 的 useCallback 依赖，若每次渲染重建则
  // handleSubmit/handleKeyDown 链随之每次重建（react-hooks/exhaustive-deps）
  const answerOf = useCallback(
    (i: number) => selected[i] ?? customs[i].trim(),
    [selected, customs],
  );
  const allAnswered = questions.every((_, i) => answerOf(i).length > 0);
  const canSubmit = allAnswered && !submitting;

  const handleSubmit = useCallback(() => {
    if (!canSubmit) return;
    const payload = {
      answers: questions.map((q, i) => ({ question: q.question, answer: answerOf(i) })),
      extra: extra.trim(),
    };
    onSubmit(JSON.stringify(payload));
    setSelected(questions.map(() => null));
    setCustoms(questions.map(() => ''));
    setExtra('');
    setTab(0);
  }, [canSubmit, questions, answerOf, extra, onSubmit]);

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
      <div className="flex items-center gap-1.5 mb-2">
        <HelpCircle className="w-4 h-4 text-amber-600 dark:text-amber-500" aria-hidden="true" />
        <span className="text-xs font-medium text-amber-700 dark:text-amber-500">AI 需要确认</span>
      </div>

      {/* 分步 tab：每个问题一个 tab + 补充信息 tab */}
      <div role="tablist" aria-label="追问分步" className="flex flex-wrap gap-1 mb-3">
        {questions.map((_, i) => (
          <button
            key={i}
            type="button"
            role="tab"
            aria-selected={tab === i}
            onClick={() => setTab(i)}
            disabled={submitting}
            className={cn(
              'px-3 py-1 rounded-full text-xs border transition-colors',
              tab === i
                ? 'border-amber-500 bg-amber-500/10 text-amber-700 dark:text-amber-500'
                : 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:border-amber-500/50',
            )}
          >
            {`问题 ${i + 1}`}
          </button>
        ))}
        <button
          type="button"
          role="tab"
          aria-selected={isExtra}
          onClick={() => setTab(EXTRA_TAB)}
          disabled={submitting}
          className={cn(
            'px-3 py-1 rounded-full text-xs border transition-colors',
            isExtra
              ? 'border-amber-500 bg-amber-500/10 text-amber-700 dark:text-amber-500'
              : 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:border-amber-500/50',
          )}
        >
          补充信息
        </button>
      </div>

      {isExtra ? (
        <textarea
          value={extra}
          onChange={(e) => setExtra(e.target.value)}
          rows={3}
          disabled={submitting}
          placeholder="补充其他信息（可选）..."
          aria-label="补充信息"
          className={cn(
            'w-full resize-none rounded-xl border border-[var(--color-border)]',
            'bg-[var(--color-bg-primary)] px-3 py-2',
            'text-sm text-[var(--color-text-primary)]',
            'placeholder:text-[var(--color-text-tertiary)]',
            'focus:outline-none focus:ring-2 focus:ring-amber-500/30 focus:border-amber-500',
            'disabled:opacity-50 disabled:cursor-not-allowed',
          )}
        />
      ) : (
        <div>
          {/* 问题文本 */}
          <p className="text-sm leading-relaxed break-words whitespace-pre-wrap mb-2">
            {question.question}
          </p>

          {/* 选项行（序号 + label + description，直接可点） */}
          {question.options.length > 0 && (
            <div role="radiogroup" aria-label="回答选项" className="flex flex-col gap-1.5 mb-2">
              {question.options.map((opt, oi) => {
                const active = selected[tab] === opt.label;
                return (
                  <button
                    key={opt.label}
                    type="button"
                    role="radio"
                    aria-checked={active}
                    onClick={() => {
                      setSelected((prev) => prev.map((v, vi) => (vi === tab ? opt.label : v)));
                      setCustoms((prev) => prev.map((v, vi) => (vi === tab ? '' : v)));
                    }}
                    disabled={submitting}
                    className={cn(
                      'flex items-start gap-2.5 px-3 py-2 rounded-xl border text-left transition-colors',
                      active
                        ? 'border-amber-500 bg-amber-500/10'
                        : 'border-[var(--color-border)] hover:border-amber-500/50',
                      'disabled:opacity-50 disabled:cursor-not-allowed',
                    )}
                  >
                    <span
                      className={cn(
                        'mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center rounded-full border text-xs',
                        active
                          ? 'border-amber-500 bg-amber-500 text-white'
                          : 'border-[var(--color-border)] text-[var(--color-text-tertiary)]',
                      )}
                    >
                      {oi + 1}
                    </span>
                    <span className="flex flex-col">
                      <span className="text-sm text-[var(--color-text-primary)]">{opt.label}</span>
                      {opt.description && (
                        <span className="text-xs text-[var(--color-text-tertiary)]">
                          {opt.description}
                        </span>
                      )}
                    </span>
                  </button>
                );
              })}
            </div>
          )}

          {/* 自定义输入（有选项时作为补充；无选项时是唯一输入） */}
          <textarea
            value={customs[tab]}
            onChange={(e) => {
              setCustoms((prev) => prev.map((v, vi) => (vi === tab ? e.target.value : v)));
              if (e.target.value.trim() !== '') {
                setSelected((prev) => prev.map((v, vi) => (vi === tab ? null : v)));
              }
            }}
            onKeyDown={handleKeyDown}
            rows={2}
            disabled={submitting}
            placeholder={
              question.options.length > 0
                ? '或输入自定义回答... (Shift+Enter 换行)'
                : '输入你的回答... (Shift+Enter 换行)'
            }
            aria-label="输入对追问的回答"
            className={cn(
              'w-full resize-none rounded-xl border border-[var(--color-border)]',
              'bg-[var(--color-bg-primary)] px-3 py-2',
              'text-sm text-[var(--color-text-primary)]',
              'placeholder:text-[var(--color-text-tertiary)]',
              'focus:outline-none focus:ring-2 focus:ring-amber-500/30 focus:border-amber-500',
              'disabled:opacity-50 disabled:cursor-not-allowed',
            )}
          />
        </div>
      )}

      {/* 提交（所有问题已回答才可提交） */}
      <div className="mt-3 flex items-center justify-end gap-2">
        <span className="text-xs text-[var(--color-text-tertiary)]">
          {allAnswered
            ? '全部问题已回答'
            : `还有 ${questions.filter((_, i) => answerOf(i).length === 0).length} 个问题未回答`}
        </span>
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
