import { useState, memo } from 'react';
import { Copy, Check, Undo2 } from 'lucide-react';
import { messageText } from '@/lib/types';
import type { ChatMessage } from '@/lib/types';
import { cn, formatTime } from '@/lib/utils';
import SkillCallCard from './SkillCallCard';
import { streamingIndicatorOwner } from './streaming-indicator';
import { SegmentBlocks } from './MessageSegments';

interface Props {
  message: ChatMessage;
  index: number;
  isStreaming: boolean;
  onRollback: (index: number) => void;
}

/** 消息气泡：用户/助手消息渲染（段渲染委托给共享 MessageSegments）。 */
function MessageBubble({ message, index, isStreaming, onRollback }: Props) {
  const [copied, setCopied] = useState(false);

  const isUser = message.role === 'user';

  // 唤醒轮空输出（allow_empty_answer：模型认为无需回复）会持久化一条
  // 空 segments 的 assistant 消息——渲染层跳过（不动数组索引，回退定位
  // 按消息 ID 的语义不受影响），避免历史中出现只有时间戳的空白气泡。
  if (!isUser && !isStreaming) {
    const hasThinking = !!message.thinking && message.thinking.length > 0;
    const hasTools = !!message.tool_calls && message.tool_calls.length > 0;
    const hasSegments = !!message.segments && message.segments.length > 0;
    if (!hasThinking && !hasTools && !hasSegments) return null;
  }

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(messageText(message));
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
            : // flex-1 + min-w-0：内容撑满剩余宽度（hover 按钮不挤压文本），
              // 同一轮内多条助手消息等宽对齐
              'flex-1 min-w-0 w-full text-[var(--color-text-primary)]',
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

        {/* 渲染路径（统一时间线）：服务端权威 segments（历史/流式同构，
            ADR-019）按真实到达顺序轮番展示思考/文本/工具调用——所有角色
            的正文都进时间线（Text 段），纯文本字段已移除（单一事实源）。 */}
        <SegmentBlocks
          segments={message.segments ?? []}
          toolCalls={message.tool_calls}
          isUser={isUser}
        />

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

        {/* 流式中断提示（finish=interrupted：网络/服务中断保留部分输出） */}
        {message.interrupted && (
          <p className="mt-2 text-base text-red-500/80 flex items-center gap-1">
            <span aria-hidden="true">⚠️</span> 流式中断，已保留部分输出
          </p>
        )}

        {/* Streaming feedback: 思考期间正文为空——显示可见的“思考中…”指示
            （含已产出思考量，避免首字符前的长静默被误认为卡住）。
            归属判定单点（streamingIndicatorOwner）：thinking 非空时气泡内接管。 */}
        {isStreaming && streamingIndicatorOwner(message) === 'bubble' && (
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
        {isStreaming && messageText(message) !== '' && (
          <div aria-live="polite" aria-atomic="true" className="sr-only">
            {messageText(message).slice(-200)}
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