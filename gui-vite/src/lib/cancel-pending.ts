/**
 * 等待会话 id 就绪后补发取消请求（停止按钮竞态修复）。
 *
 * 背景：`cancelChatStream(sessionId)` 需要会话 id，但新会话的首个聊天请求
 * 可能还没返回（`currentSessionId` 仍为 null）。此时点「停止」，取消请求
 * **发不出去**——而后端按「跑完再取」语义（断线不取消）会继续跑完并落库
 * assistant 消息；前端却已显示"已中止"，两侧状态不一致（下次加载会突然
 * 出现完整回答）。
 *
 * 本函数短轮询等待会话 id 出现后立即补发取消；超时（默认 ~10s）放弃，
 * 不产生任何副作用。返回清理函数（组件卸载或新一轮开始时调用）。
 */
export function cancelWhenSessionIdReady(
  getSessionId: () => string | null | undefined,
  sendCancel: (sessionId: string) => void,
  opts: { intervalMs?: number; maxAttempts?: number } = {},
): () => void {
  const intervalMs = opts.intervalMs ?? 250;
  const maxAttempts = opts.maxAttempts ?? 40;

  let attempts = 0;
  const timer = window.setInterval(() => {
    attempts += 1;
    const sessionId = getSessionId();
    if (sessionId) {
      window.clearInterval(timer);
      sendCancel(sessionId);
      return;
    }
    if (attempts >= maxAttempts) {
      window.clearInterval(timer);
    }
  }, intervalMs);

  return () => window.clearInterval(timer);
}
