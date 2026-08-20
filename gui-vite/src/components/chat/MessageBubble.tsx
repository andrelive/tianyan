import { useState, memo } from 'react';
import ReactMarkdown from 'react-markdown';
import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import remarkGfm from 'remark-gfm';
import { oneDark } from 'react-syntax-highlighter/dist/esm/styles/prism';
import { ChevronDown, ChevronRight, Copy, Check, Undo2 } from 'lucide-react';
import type { ChatMessage, MessageSegment } from '@/lib/types';
import { cn, formatTime } from '@/lib/utils';
import SkillCallCard from './SkillCallCard';
import ToolCallCard from './ToolCallCard';

interface Props {
  message: ChatMessage;
  index: number;
  isStreaming: boolean;
  onRollback: (index: number) => void;
}


/** 可折叠思考块。 */
function ThinkingBlock({ text }: { text: string }) {
  const [open, setOpen] = useState(true);
  return (
    <div className="mb-2 rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-tertiary)]/60 overflow-hidden">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        className="w-full flex items-center gap-1.5 px-2.5 py-1.5 text-base text-[var(--color-text-tertiary)] hover:text-[var(--color-text-secondary)] transition-colors"
      >
        {open ? (
          <ChevronDown size={12} className="shrink-0" />
        ) : (
          <ChevronRight size={12} className="shrink-0" />
        )}
        <span>思考过程</span>
      </button>
      {open && (
        <div className="px-3 pb-2 text-base leading-relaxed whitespace-pre-wrap text-[var(--color-text-secondary)] italic opacity-80 max-h-64 overflow-y-auto">
          {text}
        </div>
      )}
    </div>
  );
}

/** Markdown 正文渲染（共用样式与代码高亮）。 */
function MarkdownContent({ text, isUser }: { text: string; isUser: boolean }) {
  return (
    <div
      className={cn(
        'text-lg leading-relaxed break-words',
        '[&_p]:mb-2 [&_p:last-child]:mb-0',
        '[&_ul]:mb-2 [&_ol]:mb-2',
        '[&_ul]:pl-5 [&_ol]:pl-5',
        '[&_ul]:list-disc [&_ol]:list-decimal',
        '[&_li]:mb-0.5',
        '[&_h1]:text-xl [&_h2]:text-lg',
        '[&_h1]:font-bold [&_h2]:font-semibold',
        '[&_blockquote]:border-l-2 [&_blockquote]:border-current',
        '[&_blockquote]:pl-3 [&_blockquote]:italic [&_blockquote]:opacity-80',
        '[&_a]:underline',
        isUser ? '[&_a]:text-blue-200' : '[&_a]:text-blue-500',
        '[&_hr]:border-[var(--color-border)] [&_hr]:my-2',
      )}
    >
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          table: ({ children }) => (
            <div className="my-2 overflow-x-auto">
              <table className="min-w-full border-collapse text-lg">{children}</table>
            </div>
          ),
          th: ({ children }) => (
            <th className="border border-[var(--color-border)] px-2 py-1 text-left font-semibold bg-[var(--color-bg-tertiary)]">
              {children}
            </th>
          ),
          td: ({ children }) => (
            <td className="border border-[var(--color-border)] px-2 py-1">{children}</td>
          ),
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
                  'px-1.5 py-0.5 rounded text-lg font-mono',
                  isUser ? 'bg-blue-600/30' : 'bg-[var(--color-bg-tertiary)]',
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
        {text}
      </ReactMarkdown>
    </div>
  );
}

/** 时间线段渲染：按到达顺序轮番展示（相邻同类段合并，保持 markdown 连续）。 */
function SegmentBlocks({ segments, isUser }: { segments: MessageSegment[]; isUser: boolean }) {
  const blocks: React.ReactNode[] = [];
  let i = 0;
  let key = 0;
  while (i < segments.length) {
    const seg = segments[i];
    if (seg.type === 'thinking') {
      const texts: string[] = [];
      while (i < segments.length && segments[i].type === 'thinking') {
        const t = segments[i] as Extract<MessageSegment, { type: 'thinking' }>;
        texts.push(t.text);
        i++;
      }
      blocks.push(<ThinkingBlock key={key++} text={texts.join('')} />);
    } else if (seg.type === 'text') {
      const texts: string[] = [];
      while (i < segments.length && segments[i].type === 'text') {
        const t = segments[i] as Extract<MessageSegment, { type: 'text' }>;
        texts.push(t.text);
        i++;
      }
      blocks.push(<MarkdownContent key={key++} text={texts.join('')} isUser={isUser} />);
    } else {
      blocks.push(<ToolCallCard key={key++} event={seg.tool_call} />);
      i++;
    }
  }
  return <>{blocks}</>;
}

function MessageBubble({ message, index, isStreaming, onRollback }: Props) {
  const [copied, setCopied] = useState(false);

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

  return (
    <div className={cn('flex items-start gap-2 group', isUser ? 'flex-row-reverse' : 'flex-row')}>
      {/*
       * 用户消息：蓝色气泡；
       * 助手消息：无外层框——思考/正文/工具卡片各自独立成块平铺
       * （同轮多轮次之间无框、宽度统一，避免割裂感与宽窄不一）。
       */}
      <div
        className={cn(
          'relative',
          isUser
            ? 'max-w-[80%] rounded-2xl px-4 py-2.5 bg-blue-500 text-white rounded-br-sm'
            // flex-1 + min-w-0：内容撑满剩余宽度（hover 按钮不挤压文本），
            // 同一轮内多条助手消息等宽对齐
            : 'flex-1 min-w-0 w-full text-[var(--color-text-primary)]',
        )}
      >
        {/* 用户消息图片（data URL） */}
        {message.images && message.images.length > 0 && (
          <div className="flex flex-wrap gap-2 mb-2">
            {message.images.map((src, i) => (
              <img
                key={`${src.slice(0, 40)}-${i}`}
                src={src}
                alt={`图片 ${i + 1}`}
                loading="lazy"
                className="max-w-[240px] max-h-[240px] rounded-lg object-contain border border-white/10"
              />
            ))}
          </div>
        )}

        {/* 时间线渲染：有 segments（流式）时按事件到达顺序轮番展示
            思考/文本/工具调用；历史消息（无 segments）回退固定顺序 */}
        {message.segments && message.segments.length > 0 ? (
          <SegmentBlocks segments={message.segments} isUser={isUser} />
        ) : (
          <>
            {/* 思考过程（可折叠，与正文分开渲染，按序轮番出现） */}
            {!isUser && message.thinking && (
              <ThinkingBlock text={message.thinking} />
            )}

            <MarkdownContent text={message.content} isUser={isUser} />

            {/* Tool calls（A2 展示契约：按展示意图渲染卡片；调用与结果合并，
                结果完整内容折叠展示，不截断） */}
            {message.tool_calls && message.tool_calls.length > 0 && (
              <div className="mt-2 space-y-1">
                {message.tool_calls.map((call, i) => (
                  <ToolCallCard
                    key={`${call.name}-${call.arguments}-${i}`}
                    event={call}
                    result={call.result}
                  />
                ))}
              </div>
            )}
          </>
        )}

        {/* Skill calls */}
        {message.skill_calls && message.skill_calls.length > 0 && (
          <div className="mt-2 space-y-1">
            {message.skill_calls.map((call, i) => (
              <SkillCallCard key={`${call.skill_id}-${i}`} info={call} />
            ))}
          </div>
        )}

        {/* 输出达到 token 上限被截断的提示（finish_reason === 'length'） */}
        {message.truncated_by_length && (
          <p className="mt-2 text-base text-[var(--color-text-tertiary)] flex items-center gap-1">
            <span aria-hidden="true">⚠️</span> 输出已达上限
          </p>
        )}

        {/* Streaming feedback: 思考期间正文为空——显示可见的“思考中…”指示
            （含已产出思考量，避免首字符前的长静默被误认为卡住） */}
        {isStreaming && message.content === '' && (
          <span
            className="inline-flex items-center gap-1.5 text-base text-[var(--color-text-tertiary)] animate-pulse"
            aria-label="AI 正在思考中..."
          >
            <span className="inline-block w-2 h-2 bg-current rounded-full" />
            思考中
            {message.thinking && message.thinking.length > 0
              ? ` · ${message.thinking.length} 字`
              : '…'}
          </span>
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
              'text-base mt-1 opacity-60',
              isUser ? 'text-right text-blue-100' : 'text-left text-[var(--color-text-tertiary)]',
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
          isUser ? 'flex-row' : 'flex-row',
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

        {/* Rollback - 仅用户消息：回退到该用户输入之前（删除该消息及其后的
            所有内容——包括大模型的多轮输出；不对 assistant 输出提供回退，
            避免回退到模型单轮输出这种无意义粒度） */}
        {!isStreaming && isUser && (
          <button
            onClick={() => onRollback(index)}
            className="p-1.5 rounded-md hover:bg-red-50 dark:hover:bg-red-950/30 text-[var(--color-text-tertiary)] hover:text-red-500 transition-colors"
            title="回退到此（删除该消息及其后的所有内容）"
            aria-label="回退到此"
          >
            <Undo2 className="w-3.5 h-3.5" />
          </button>
        )}
      </div>
    </div>
  );
}

export default memo(MessageBubble);
