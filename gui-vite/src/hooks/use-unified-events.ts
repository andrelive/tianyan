/**
 * useUnifiedEvents —— 统一事件订阅（ADR-028）。
 *
 * 前端启动即建立常驻 EventSource（GET /events），事件带 type 区分：
 * - `message`：会话消息（落库即推送，带 session_id + seq）→ mergeServerMessages
 *   按 id 去重合并到对应会话（追加语义，保留本地流式累积）；
 * - `task_status`：任务状态转移（pending/running/终态）→ 任务面板实时更新；
 * - `command_output`：命令输出增量（尽力而为，终端视图实时 append）。
 *
 * 可靠性分层：
 * - 消息类永久可靠：落库 + 重连即快照（挂载 fetch 全量）+ 断点对齐兜底；
 * - 任务实时类尽力而为：通道丢事件可接受，日志文件兜底。
 *
 * 断线重连：EventSource 自动重连；重连后前端重新 fetch 快照（replace），
 * 与数据库收敛到一致（DSH 模式，不使用 Last-Event-ID）。
 */

import { useEffect, useRef } from 'react';
import { getApiBase } from '@/lib/api-base';
import { useAppStore } from '@/lib/store';
import { fetchSessionMessages } from '@/lib/api-client';
import type { ChatMessage } from '@/lib/types';

/** 统一事件（ADR-028 事件契约）。 */
export interface UnifiedEvent {
  type: 'message' | 'task_status' | 'command_output';
  /** message 事件：会话 ID。 */
  session_id?: string;
  /** message 事件：持久化序号（断点对齐锚点）。 */
  seq?: number;
  /** message 事件：完整消息。 */
  message?: ChatMessage;
  /** task_status / command_output 事件：任务 ID。 */
  task_id?: string;
  /** task_status 事件：任务状态。 */
  status?: string;
  /** command_output 事件：输出增量。 */
  delta?: string;
  [key: string]: unknown;
}

/** 事件订阅回调（组件按需注册；返回取消函数）。 */
export type EventHandler = (ev: UnifiedEvent) => void;

/**
 * 全局统一事件订阅（单例 EventSource，按 type 分发）。
 *
 * 组件通过 useUnifiedEvents(handler) 注册回调；hook 内部维护单例连接，
 * 首个订阅者建立、最后一个退订时关闭。消息类事件（type=message）由
 * 本 hook 直接合并进 store（mergeServerMessages），组件无需重复处理。
 */
export function useUnifiedEvents(handler?: EventHandler): void {
  const handlerRef = useRef<EventHandler | null>(null);
  handlerRef.current = handler ?? null;

  useEffect(() => {
    const listeners = unifiedEventsListeners;
    listeners.add(handlerRef);
    if (!unifiedEventsStarted) {
      unifiedEventsStarted = true;
      startUnifiedEvents();
    }
    return () => {
      listeners.delete(handlerRef);
      if (listeners.size === 0 && unifiedEventsStarted) {
        unifiedEventsStarted = false;
        stopUnifiedEvents();
      }
    };
  }, []);
}

// ── 单例连接管理 ─────────────────────────────────────────────

const unifiedEventsListeners = new Set<React.MutableRefObject<EventHandler | null>>();
let unifiedEventsStarted = false;
let es: EventSource | null = null;

function startUnifiedEvents(): void {
  if (typeof EventSource === 'undefined') return; // 测试环境（jsdom）无 EventSource
  es = new EventSource(getApiBase() + '/events');
  es.onmessage = (e) => {
    try {
      const ev = JSON.parse(e.data) as UnifiedEvent;
      if (ev.type === 'message' && ev.session_id && ev.message) {
        // 消息类：直接合并进 store（追加语义，按 id 去重）
        useAppStore.getState().mergeServerMessages(ev.session_id, [ev.message]);
      }
      for (const ref of unifiedEventsListeners) {
        ref.current?.(ev);
      }
    } catch {
      /* 畸形事件跳过 */
    }
  };
  es.onerror = () => {
    // EventSource 自动重连；重连后前端重新 fetch 快照（DSH 模式）。
    // 断线期间消息已落库，重连后由挂载/切换会话的 fetch 全量兜底。
  };
}

function stopUnifiedEvents(): void {
  es?.close();
  es = null;
}

/** 断点对齐：重连/跳号后从数据库拉取快照替换本地（DSH replace 语义）。 */
export async function alignSessionFromServer(sessionId: string): Promise<void> {
  try {
    const data = await fetchSessionMessages(sessionId);
    useAppStore.getState().setSessionMessages(sessionId, data.messages);
  } catch {
    /* 对齐失败静默（下次重连/挂载再试） */
  }
}
