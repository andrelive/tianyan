import { useState, useEffect, memo } from 'react';
import ReactMarkdown from 'react-markdown';
import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import { oneDark } from 'react-syntax-highlighter/dist/esm/styles/prism';
import {
  Copy,
  Check,
  Edit3,
  RotateCcw,
  Trash2,
} from 'lucide-react';
import type { ChatMessage } from '@/lib/types';
import { cn, formatTime } from '@/lib/utils';
import SkillCallCard from './SkillCallCard';

interface Props {
  message: ChatMessage;
  index: number;
  isStreaming: boolean;
  onRegenerate: (index: number) => void;
  onEdit: (index: number, newContent: string) => void;
  onDelete: (index: number) => void;
}

function MessageBubble({
  message,
  index,
  isStreaming,
  onRegenerate,
  onEdit,
  onDelete,
}: Props) {
  const [isEditing, setIsEditing] = useState(false);
  const [editContent, setEditContent] = useState(message.content);
  const [copied, setCopied] = useState(false);

  // Sync editContent with message.content when not actively editing
  useEffect(() => {
    if (!isEditing) {
      setEditContent(message.content);
    }
  }, [message.content, isEditing]);

  const isUser = message.role === 'user';

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(message.content);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      // Clipboard API may fail in some contexts
    }
  };

  const handleEditSave = () => {
    const trimmed = editContent.trim();
    if (trimmed && trimmed !== message.content) {
      onEdit(index, trimmed);
    }
    setIsEditing(false);
  };

  const handleEditCancel = () => {
    setIsEditing(false);
    setEditContent(message.content);
  };

  const handleEditKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleEditSave();
    }
    if (e.key === 'Escape') {
      handleEditCancel();
    }
  };

  return (
    <div
      className={cn(
        'flex items-start gap-2 group',
        isUser ? 'flex-row-reverse' : 'flex-row'
      )}
    >
      {/* Bubble */}
      <div
        className={cn(
          'max-w-[80%] rounded-2xl px-4 py-2.5 relative',
          isUser
            ? 'bg-blue-500 text-white rounded-br-sm'
            : 'bg-[var(--color-bg-secondary)] text-[var(--color-text-primary)] rounded-bl-sm border border-[var(--color-border)]'
        )}
      >
        {isEditing ? (
          <div className="space-y-2 min-w-[300px]">
            <textarea
              value={editContent}
              onChange={(e) => setEditContent(e.target.value)}
              onKeyDown={handleEditKeyDown}
              className="w-full bg-white dark:bg-[var(--color-bg-tertiary)] border border-blue-300 dark:border-blue-600 rounded-lg p-2 text-sm text-[var(--color-text-primary)] focus:outline-none focus:ring-2 focus:ring-blue-500 resize-none"
              rows={4}
              autoFocus
              aria-label="编辑消息内容"
            />
            <div className="flex gap-2 justify-end">
              <button
                onClick={handleEditCancel}
                className="px-3 py-1 text-xs rounded-md bg-[var(--color-bg-tertiary)] hover:bg-[var(--color-bg-hover)] text-[var(--color-text-secondary)] transition-colors"
                aria-label="取消编辑"
              >
                取消
              </button>
              <button
                onClick={handleEditSave}
                className="px-3 py-1 text-xs rounded-md bg-blue-500 text-white hover:bg-blue-600 transition-colors"
                aria-label="保存编辑"
              >
                保存
              </button>
            </div>
          </div>
        ) : (
          <div
            className={cn(
              'text-sm leading-relaxed break-words',
              '[&_p]:mb-2 [&_p:last-child]:mb-0',
              '[&_ul]:mb-2 [&_ol]:mb-2',
              '[&_ul]:pl-5 [&_ol]:pl-5',
              '[&_ul]:list-disc [&_ol]:list-decimal',
              '[&_li]:mb-0.5',
              '[&_h1]:text-base [&_h2]:text-sm',
              '[&_h1]:font-bold [&_h2]:font-semibold',
              '[&_blockquote]:border-l-2 [&_blockquote]:border-current',
              '[&_blockquote]:pl-3 [&_blockquote]:italic [&_blockquote]:opacity-80',
              '[&_a]:underline',
              isUser ? '[&_a]:text-blue-200' : '[&_a]:text-blue-500',
              '[&_hr]:border-[var(--color-border)] [&_hr]:my-2'
            )}
          >
            <ReactMarkdown
              components={{
                code: ({ className, children, ...props }: React.ComponentPropsWithoutRef<'code'>) => {
                  const match = /language-(\w+)/.exec(className || '');
                  const codeString = String(children).replace(/\n$/, '');
                  if (match) {
                    return (
                      <SyntaxHighlighter
                        style={oneDark}
                        language={match[1]}
                        PreTag="div"
                        customStyle={{
                          borderRadius: '8px',
                          fontSize: '0.8rem',
                          margin: '0.5rem 0',
                        }}
                      >
                        {codeString}
                      </SyntaxHighlighter>
                    );
                  }
                  return (
                    <code
                      className={cn(
                        'px-1.5 py-0.5 rounded text-sm font-mono',
                        isUser
                          ? 'bg-blue-600/30'
                          : 'bg-[var(--color-bg-tertiary)]'
                      )}
                      {...props}
                    >
                      {children}
                    </code>
                  );
                },
                pre: ({ children }: React.ComponentPropsWithoutRef<'pre'>) => {
                  return <>{children}</>;
                },
              }}
            >
              {message.content}
            </ReactMarkdown>
          </div>
        )}

        {/* Skill calls */}
        {message.skill_calls && message.skill_calls.length > 0 && (
          <div className="mt-2 space-y-1">
            {message.skill_calls.map((call, i) => (
              <SkillCallCard key={`${call.skill_id}-${i}`} info={call} />
            ))}
          </div>
        )}

        {/* Streaming cursor for empty content */}
        {isStreaming && message.content === '' && (
          <span className="inline-block w-2 h-4 bg-current animate-pulse rounded-sm" aria-label="AI 正在思考中..." />
        )}

        {/* Live region for streaming content updates */}
        {isStreaming && message.content !== '' && (
          <div aria-live="polite" aria-atomic="true" className="sr-only">
            {message.content.slice(-200)}
          </div>
        )}

        {/* Timestamp */}
        {message.timestamp && (
          <p
            className={cn(
              'text-xs mt-1 opacity-60',
              isUser ? 'text-right text-blue-100' : 'text-left text-[var(--color-text-tertiary)]'
            )}
          >
            {formatTime(message.timestamp)}
          </p>
        )}
      </div>

      {/* Hover actions */}
      <div
        className={cn(
          'flex gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity pt-1',
          isUser ? 'flex-row' : 'flex-row'
        )}
      >
        {/* Copy - both roles */}
        <button
          onClick={handleCopy}
          className="p-1.5 rounded-md hover:bg-[var(--color-bg-hover)] text-[var(--color-text-tertiary)] hover:text-[var(--color-text-primary)] transition-colors"
          title={copied ? '已复制' : '复制'}
          aria-label={copied ? '已复制' : '复制消息'}
        >
          {copied ? (
            <Check className="w-3.5 h-3.5 text-green-500" />
          ) : (
            <Copy className="w-3.5 h-3.5" />
          )}
        </button>

        {/* Edit - user only */}
        {isUser && !isStreaming && (
          <button
            onClick={() => {
              setEditContent(message.content);
              setIsEditing(true);
            }}
            className="p-1.5 rounded-md hover:bg-[var(--color-bg-hover)] text-[var(--color-text-tertiary)] hover:text-[var(--color-text-primary)] transition-colors"
            title="编辑"
            aria-label="编辑消息"
          >
            <Edit3 className="w-3.5 h-3.5" />
          </button>
        )}

        {/* Regenerate - assistant only */}
        {!isUser && !isStreaming && (
          <button
            onClick={() => onRegenerate(index)}
            className="p-1.5 rounded-md hover:bg-[var(--color-bg-hover)] text-[var(--color-text-tertiary)] hover:text-[var(--color-text-primary)] transition-colors"
            title="重新生成"
            aria-label="重新生成回复"
          >
            <RotateCcw className="w-3.5 h-3.5" />
          </button>
        )}

        {/* Delete - both roles */}
        {!isStreaming && (
          <button
            onClick={() => onDelete(index)}
            className="p-1.5 rounded-md hover:bg-red-50 dark:hover:bg-red-950/30 text-[var(--color-text-tertiary)] hover:text-red-500 transition-colors"
            title="删除"
            aria-label="删除消息"
          >
            <Trash2 className="w-3.5 h-3.5" />
          </button>
        )}
      </div>
    </div>
  );
}

export default memo(MessageBubble);
