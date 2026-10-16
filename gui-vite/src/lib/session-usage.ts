/**
 * 会话 token 汇总刷新（底部统计条）。
 *
 * 数据源 = 后端 usage_logs 按 session_id 全量聚合——与消息分页窗口解耦：
 * 此前本地累加已加载 messages，分页首载（最近 50 条）后数字随加载量
 * 缩水（观感「变成当前上下文」）。
 */

import { fetchSessionTokenUsage } from '@/lib/api-client';
import { useAppStore } from '@/lib/store';

/** 拉取并写入会话汇总；全零/失败静默保留旧值（统计尽力而为，不打扰会话流）。 */
export async function refreshSessionTokenUsage(sessionId: string): Promise<void> {
  try {
    const usage = await fetchSessionTokenUsage(sessionId);
    if (usage) useAppStore.getState().setSessionTokenUsage(sessionId, usage);
  } catch {
    // 静默：统计数据不阻塞会话
  }
}
