/**
 * token 用量纯函数（从 ChatPanel 下沉——渲染层不再内联聚合逻辑）。
 *
 * 两个函数均为 messages → 摘要的纯映射，独立可测；
 * 组件侧只需 useMemo 包装（输入不变不重算）。
 */

import type { ChatMessage, StreamUsage } from '@/lib/types';

/**
 * 最近一轮完成的上下文占用：从末尾向前找第一条带 usage 的消息。
 * 压缩摘要消息（compression_marker）特殊处理——其 prompt_tokens 是压缩
 * 请求的输入（≈ 压缩前上下文），不能直接作占用；改用「第一条 assistant
 * 的 prompt_tokens（≈系统前缀）+ 摘要输出」估算压缩后上下文。
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
      // 压缩摘要消息：其 prompt_tokens 是压缩请求输入（≈ 压缩前上下文），
      // 不能直接作占用——压缩后上下文 ≈ 系统前缀（第一条 assistant 输入）
      // + 摘要输出（真实压缩请求 completion）。压缩后未产生新请求时，
      // 这是唯一能立即反映压缩后占用的来源；新请求完成后新 assistant
      // 消息优先命中，摘要不再被取到。
      if (messages[i].compression_marker) {
        const firstAssistant = messages.find(
          (m) => m.role === 'assistant' && (m.usage?.prompt_tokens ?? 0) > 0,
        );
        const systemPrefix = firstAssistant?.usage?.prompt_tokens ?? 0;
        const completion = u.completion_tokens ?? 0;
        return {
          prompt_tokens: systemPrefix + completion,
          completion_tokens: completion,
          total_tokens: systemPrefix + completion,
          cache_read: u.cache_read ?? 0,
          cache_write: u.cache_write ?? 0,
          context_window: fallbackWindow || 0,
        };
      }
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
    // 压缩摘要消息自带压缩请求真实用量——与 assistant 消息同样累加
    // （压缩不走 AgentLoop，其消耗须入账；prompt_tokens 是压缩前上下文
    // 重发，按真实消耗计）
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
