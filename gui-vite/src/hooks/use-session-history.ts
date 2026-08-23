/**
 * useSessionHistory —— 会话消息历史加载（挂载恢复 + 失败重载）的唯一入口。
 *
 * ChatPanel 此前在组件内联 3 段历史获取（挂载加载 / reloadSession / 唤醒轮询
 * 各自 fetch）——端点契约与缓存守卫知识散落。本 hook 收敛：
 * - 挂载时按 URL 会话加载一次（本地已有缓存则跳过；StrictMode 双跑安全；
 *   应用前校验当前 URL 仍指向该会话，防旧请求覆盖）；
 * - reloadSession：从服务端权威结构覆盖指定会话缓存（失败回退空列表——
 *   与持久化状态保持一致）。
 */

import { useCallback, useEffect, useRef } from 'react';
import { fetchSessionMessages } from '@/lib/api-client';
import { useAppStore } from '@/lib/store';

export function useSessionHistory(urlSessionId: string | null | undefined) {
  const loadedOnMountRef = useRef(false);
  const activeUrlSessionRef = useRef<string | null>(null);

  useEffect(() => {
    activeUrlSessionRef.current = urlSessionId ?? null;
  }, [urlSessionId]);

  // 刷新/直达 URL（如 /chat/{id}）时恢复历史消息：仅在挂载时执行一次。
  // 列表点击路径由 SessionList.handleSelectSession 负责加载（导航后
  // currentSessionId 已一致，此处不会重复请求）。
  // StrictMode 双跑安全：ref 守卫只允许一次；cleanup 不取消请求（取消会
  // 杀死唯一请求），改由「应用前校验当前 URL 仍指向该会话」防旧请求覆盖。
  useEffect(() => {
    if (loadedOnMountRef.current) return;
    loadedOnMountRef.current = true;
    if (!urlSessionId) return;
    const sessionId = urlSessionId;
    (async () => {
      // 本地已有缓存（流式累积/之前看过）→ 直接显示，不重复拉历史
      if (useAppStore.getState().hasSessionMessages(sessionId)) return;
      try {
        const data = await fetchSessionMessages(sessionId);
        if (activeUrlSessionRef.current === sessionId) {
          useAppStore.getState().setMessages(data.messages);
        }
      } catch {
        // 加载失败保持空列表（与 reloadSession 行为一致）
      }
    })();
  }, [urlSessionId]);

  // 从后端重新加载会话消息（失败回滚，恢复与持久化一致的状态）。
  // 按目标会话更新缓存：流归属会话非当前会话时也只写对应字典，不污染当前投影。
  const reloadSession = useCallback(async (sessionId: string) => {
    try {
      const data = await fetchSessionMessages(sessionId);
      useAppStore.getState().setSessionMessages(sessionId, data.messages);
    } catch {
      useAppStore.getState().setSessionMessages(sessionId, []);
    }
  }, []);

  return { reloadSession };
}
