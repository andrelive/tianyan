/**
 * useSessionHistory / loadSessionHistory —— 会话消息历史加载的唯一入口。
 *
 * ChatPanel 此前在组件内联 3 段历史获取（挂载加载 / reloadSession / 唤醒轮询
 * 各自 fetch）——端点契约与缓存守卫知识散落。本 hook 收敛：
 * - 按会话切换订阅（ADR-029：subscribeSession——快照帧经统一事件通道到达，
 *   replace 窗口；已订阅（resident）幂等跳过，切回零延迟；**历史未加载时
 *   强制重发**，否则流式残留的旧缓存会把历史永久挡住）；
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
  // ADR-035 §8 修订：**缓存非空 ≠ 历史已加载**——流式事件（唤醒轮/后台轮）
  // 会按 session_id 向未打开的会话写入本轮消息（chat-stream.ts），却从不写
  // 分页元数据。若把这种缓存当成"历史已加载"，点击打开时就会跳过历史拉取，
  // 界面只剩流式残留的少量消息，且无 meta → 上滚开关恒关（用户报告：定时
  // 任务会话只显示 1 条、看不到历史、不能上滚）。
  // 判据改为「有缓存 **且** 有分页元数据」。
  if (st.hasSessionMessages(sessionId) && st.sessionMessageMeta[sessionId] != null) return true;
  try {
    const data = await fetchSessionMessagesPage(sessionId, undefined, PAGE_SIZE);
    const cur = useAppStore.getState();
    if ((cur.streamStatus[sessionId] ?? 'idle') === 'streaming') return true;
    // 请求期间已有权威分页元数据（快照/其它路径先到）→ 丢弃迟到响应
    if (cur.sessionMessageMeta[sessionId] != null) return true;
    const meta = {
      oldestSeq: data.next_before_seq ?? null,
      hasMore: Boolean(data.has_more),
    };
    // 历史页是**权威窗口**：非运行态直接用服务端页 replace。
    //
    // 修复（实测 bug：回答结束后尾部又重复一遍"思考 + 工具卡片"）：
    // 流式产生的消息 `seq = null`（服务端边界事件不带 seq），若走合并，它们
    // 会被当作"无 seq 的流式占位"保留在尾部——与历史页里**同一内容**（有
    // seq）并存 → 重复展示。非运行态下本地内容都已落库（历史页已含），
    // replace 才正确。
    //
    // 唯一例外：auto 轮（唤醒轮/后台轮）运行中**不设** streamStatus（上面的
    // 守卫拦不到），本地可能含尚未落库的输出 → 保留合并语义，避免丢半截输出。
    const turnRunning = (cur.turnState[sessionId]?.state ?? 'idle') === 'running';
    if (cur.hasSessionMessages(sessionId) && turnRunning) {
      cur.mergeSessionMessages(sessionId, data.messages, meta);
    } else {
      cur.setSessionMessages(sessionId, data.messages);
      cur.setSessionMessageMeta(sessionId, meta);
    }
    return true;
  } catch {
    return false;
  }
}

export function useSessionHistory(urlSessionId: string | null | undefined) {
  /** 已强制重订阅过的会话（StrictMode 双挂载/同会话重复渲染去重）。 */
  const forcedRef = useRef(new Set<string>());

  // 会话切换即订阅（ADR-029）：快照帧（完整历史尾部 + has_more + 轮状态
  // 权威值）经统一事件通道 replace 窗口，是历史的权威兜底。
  //
  // 修复（历史永远缺失）：此前用「仅挂载执行一次」的 ref 守卫——但
  // `/chat/{A}` → `/chat/{B}` 同路由只变参数、组件不重挂载，守卫已消费
  // → B 永不订阅（无快照），叠加"缓存非空即已加载"的误判，历史永久缺失。
  // 现在按 `urlSessionId` 变化订阅；**历史未加载（无 meta）时强制重发**
  // （resident 幂等会挡住正常重发，导致流式残留的旧缓存永远得不到快照）。
  useEffect(() => {
    if (!urlSessionId) return;
    const hasHistory = useAppStore.getState().sessionMessageMeta[urlSessionId] != null;
    const needForce = !hasHistory && !forcedRef.current.has(urlSessionId);
    if (needForce) forcedRef.current.add(urlSessionId);
    void subscribeSession(urlSessionId, { force: needForce });
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
