import { useState, useRef, useCallback, useEffect } from 'react';
import { Send, Square } from 'lucide-react';
import { cn } from '@/lib/utils';

interface Props {
  onSend: (content: string) => void;
  onStop: () => void;
  isStreaming: boolean;
}

export default function ChatInput({ onSend, onStop, isStreaming }: Props) {
  const [input, setInput] = useState('');
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // Auto-resize textarea as content grows
  useEffect(() => {
    const el = textareaRef.current;
    if (el) {
      el.style.height = 'auto';
      el.style.height = Math.min(el.scrollHeight, 200) + 'px';
    }
  }, [input]);

  // Focus on mount
  useEffect(() => {
    textareaRef.current?.focus();
  }, []);

  const handleSend = useCallback(() => {
    if (isStreaming || !input.trim()) return;
    onSend(input);
    setInput('');
    if (textareaRef.current) {
      textareaRef.current.style.height = 'auto';
    }
  }, [input, isStreaming, onSend]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        handleSend();
      }
    },
    [handleSend],
  );

  return (
    <div className="border-t border-[var(--color-border)] p-4 bg-[var(--color-bg-primary)]">
      <div className="flex items-end gap-3 max-w-4xl mx-auto">
        <div className="relative flex-1">
          <textarea
            ref={textareaRef}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="输入消息... (Shift+Enter 换行)"
            rows={1}
            disabled={isStreaming}
            aria-label="输入消息"
            className={cn(
              'w-full resize-none rounded-xl border border-[var(--color-border)]',
              'bg-[var(--color-bg-secondary)] px-4 py-3',
              'text-sm text-[var(--color-text-primary)]',
              'placeholder:text-[var(--color-text-tertiary)]',
              'focus:outline-none focus:ring-2 focus:ring-blue-500/30 focus:border-blue-500',
              'disabled:opacity-50 disabled:cursor-not-allowed',
              'transition-colors',
            )}
          />
        </div>

        {isStreaming ? (
          <button
            onClick={onStop}
            className="flex items-center gap-2 px-4 py-3 rounded-xl bg-red-500 hover:bg-red-600 text-white text-sm font-medium transition-colors shrink-0"
            aria-label="停止生成"
          >
            <Square className="w-4 h-4 fill-current" />
            停止
          </button>
        ) : (
          <button
            onClick={handleSend}
            disabled={!input.trim()}
            aria-label="发送消息"
            className={cn(
              'flex items-center gap-2 px-4 py-3 rounded-xl text-white text-sm font-medium transition-colors shrink-0',
              input.trim()
                ? 'bg-blue-500 hover:bg-blue-600'
                : 'bg-[var(--color-bg-tertiary)] text-[var(--color-text-tertiary)] cursor-not-allowed',
            )}
          >
            <Send className="w-4 h-4" />
            发送
          </button>
        )}
      </div>
    </div>
  );
}
