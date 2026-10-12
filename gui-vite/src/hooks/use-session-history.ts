/**
 * useSessionHistory / loadSessionHistory —— 会话消息历史加载的唯一入口。
 *
 * ChatPanel 此前在组件内联 3 段历史获取（挂载加载 / reloadSession / 唤醒轮询
 * 各自 fetch）——端点契约与缓存守卫知识散落。本 hook 收敛：
 * - 挂载时按 URL 会话订阅（ADR-029：subscribeSession——快照帧经统一事件
 *   通道到达，replace 窗口；已订阅（resident）幂等跳过，切回零延迟）；
 * - reloadSession：从服务端权威结构覆盖指定会话缓存（失败回退空列表——
 *   与持久化状态保持一致）。
 */

import { useCallback, useEffect, useRef } from 'react';
import { fetchSessionMessagesPage } from '@/lib/api-client';
import { useAppStore } from '@/lib/store';
import { subscribeSession } from '@/hooks/use-unified-events';

/** 首载与上滚共用的页大小（对齐服务端默认；上限 200）。 */
const PAGE_SIZE = 50;

/**
 * 懒加载会话历史（唯一入口，SessionList 与其它调用方共用）：按会话 id
 * 寻址写入——迟到响应写入发起会话，不污染中途切换到的会话；请求期间
 * 已被快照/流式写入（双重检查）或该会话流式中（与 `applySnapshot`
 * 同规则）时丢弃迟到响应。
 *
 * ADR-035 §8：首载只取最近一页（`PAGE_SIZE` 条）+ 分页元数据（上滚游标
 * `next_before_seq` / `has_more`）——更早历史由 `loadOlder` 上滚补齐；
 * 大会话不再全量拉取（全量可到数十 MB / 上千条，是切换卡顿的根源）。
 *
 * 返回可用性：已有缓存 / 加载成功 → true；网络失败 → false（调用方
 * 决定是否提示）。
 */
export async function loadSessionHistory(sessionId: string): Promise<boolean> {
  const st = useAppStore.getState();
  if (st.hasSessionMessages(sessionId)) return true;
  try {
    const data = await fetchSessionMessagesPage(sessionId, undefined, PAGE_SIZE);
    const cur = useAppStore.getState();
    if (cur.hasSessionMessages(sessionId)) return true;
    if ((cur.streamStatus[sessionId] ?? 'idle') === 'streaming') return true;
    cur.setSessionMessages(sessionId, data.messages);
    cur.setSessionMessageMeta(sessionId, {
      oldestSeq: data.next_before_seq ?? null,
      hasMore: Boolean(data.has_more),
    });
    return true;
  } catch {
    return false;
  }
}

export function useSessionHistory(urlSessionId: string | null | undefined) {
  const loadedOnMountRef = useRef(false);

  // 刷新/直达 URL（如 /chat/{id}）时恢复历史消息：仅在挂载时执行一次。
  // 列表点击路径由 SessionList.handleSelectSession 负责加载（导航后
  // currentSessionId 已一致，此处不会重复请求）。
  // StrictMode 双跑安全：ref 守卫只允许一次。
  useEffect(() => {
    if (loadedOnMountRef.current) return;
    loadedOnMountRef.current = true;
    if (!urlSessionId) return;
    // ADR-029：订阅即快照恢复——快照帧（完整历史 + cursor）经统一事件
    // 通道到达，useUnifiedEvents 按 sid replace 窗口。已订阅（resident）
    // 幂等跳过；未订阅首次打开 → 快照加载。不再 fetch（快照与实时事件
    // 同一条流，合并竞态消失）。
    void subscribeSession(urlSessionId);
  }, [urlSessionId]);

  // 从后端重新加载会话消息（失败回滚，恢复与持久化一致的状态）。
  // ADR-035 §8：取最近一页 + 分页元数据（与首载同规格，可继续上滚）；
  // 按目标会话更新缓存：流归属会话非当前会话时也只写对应字典，不污染当前投影。
  const reloadSession = useCallback(async (sessionId: string) => {
    try {
      const data = await fetchSessionMessagesPage(sessionId, undefined, PAGE_SIZE);
      const st = useAppStore.getState();
      st.setSessionMessages(sessionId, data.messages);
      st.setSessionMessageMeta(sessionId, {
        oldestSeq: data.next_before_seq ?? null,
        hasMore: Boolean(data.has_more),
      });
    } catch {
      const st = useAppStore.getState();
      st.setSessionMessages(sessionId, []);
      st.setSessionMessageMeta(sessionId, { oldestSeq: null, hasMore: false });
    }
  }, []);

  /**
   * 上滚：加载更早一页（ADR-035 §8）。
   *
   * 分页元数据由快照帧（打开会话）或本函数（上滚）维护；到链首
   * （hasMore=false 或 oldestSeq<=0）后不再请求。
   * 返回本次加载的条数（0 = 未加载——无更早历史/失败）；调用方（虚拟列表）
   * 据此递减 firstItemIndex 保持前插锚定。
   */
  const loadOlder = useCallback(async (sessionId: string): Promise<number> => {
    const st = useAppStore.getState();
    const meta = st.sessionMessageMeta[sessionId];
    if (!meta || !meta.hasMore || meta.oldestSeq == null || meta.oldestSeq <= 0) {
      return 0;
    }
    try {
      const data = await fetchSessionMessagesPage(sessionId, meta.oldestSeq, PAGE_SIZE);
      useAppStore.getState().prependSessionMessages(sessionId, data.messages, {
        oldestSeq: data.next_before_seq ?? null,
        hasMore: Boolean(data.has_more),
      });
      return data.messages.length;
    } catch {
      return 0;
    }
  }, []);

  return { reloadSession, loadOlder };
}
