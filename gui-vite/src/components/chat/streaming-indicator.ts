/**
 * 流式思考指示器归属（单点判定，替代 ChatPanel/MessageBubble 的注释互斥约定）：
 *
 * - `bubble`：thinking 非空 → 气泡内接管（「思考中 · N 字」）；
 * - `list`：thinking 为空 → 列表底部指示器（「思考中...」）；
 * - `null`：正文已产出或非流式 → 两者都不显示。
 *
 * 互斥由本函数保证：任一调用方改判定条件，另一侧自动跟随，不会双显/都不显。
 */
export function streamingIndicatorOwner(message: {
  content: string;
  thinking?: string | null;
}): 'bubble' | 'list' | null {
  if (message.content !== '') return null;
  return message.thinking ? 'bubble' : 'list';
}
