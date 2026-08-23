/**
 * token 用量纯函数（从 ChatPanel 下沉——渲染层不再内联聚合逻辑）。
 *
 * 两个函数均为 messages → 摘要的纯映射，独立可测；
 * 组件侧只需 useMemo 包装（输入不变不重算）。
 */

import type { ChatMessage, StreamUsage } from '@/lib/types';

/**
 * 最近一轮完成的上下文占用：从末尾向前找第一条带 usage 的消息。
 * 无则 null（调用方决定展示与否）。
 *
 * @param fallbackWindow 模型声明的上下文窗口（无真实窗口记录时兜底）
 */
export function lastMessageUsage(
  messages: ChatMessage[],
  fallbackWindow: number,
): StreamUsage | null {
  for (let i = messages.length - 1; i >= 0; i--) {
    const u = messages[i].usage;
    if (u && u.prompt_tokens > 0) {
      return {
        prompt_tokens: u.prompt_tokens,
        completion_tokens: u.completion_tokens,
        total_tokens: u.total_tokens,
        cache_read: u.cache_read ?? 0,
        cache_write: u.cache_write ?? 0,
        context_window: fallbackWindow || 0,
      };
    }
  }
  return null;
}

/** 会话 token 消耗汇总（跨全部消息 usage 累加；全零时返回 null）。 */
export function sumSessionUsage(
  messages: ChatMessage[],
): { uncachedInput: number; cachedInput: number; completion: number } | null {
  let uncachedInput = 0;
  let cachedInput = 0;
  let completion = 0;
  for (const m of messages) {
    const u = m.usage;
    if (!u || u.prompt_tokens <= 0) continue;
    const cached = u.cache_read ?? 0;
    cachedInput += cached;
    uncachedInput += Math.max(0, u.prompt_tokens - cached);
    completion += u.completion_tokens;
  }
  if (uncachedInput + cachedInput + completion === 0) return null;
  return { uncachedInput, cachedInput, completion };
}
