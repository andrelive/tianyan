/**
 * useUnifiedEvents —— 统一事件订阅（ADR-028/031）。
 *
 * 前端启动即建立常驻 EventSource（GET /events），事件带 type 区分：
 * - `chat_stream`：对话流式事件（ADR-028 第 3 步）→ 按 session_id 路由到
 *   活跃归约器（主会话 / 子代理 task_id 同构）；
 * - `snapshot`：订阅快照（ADR-029）→ replace 窗口（打开/重连的权威兜底）；
 * - `task_status`：任务状态转移（pending/running/终态）→ 任务面板实时更新；
 * - `command_output`：命令输出增量（尽力而为，终端视图实时 append）。
 *
 * ADR-031 后消息不再广播（落库即广播移除）：主会话消息经流式增量 + 完成
 * 事件（user_message_id 确认 / pump 边界）到前端；后台任务完成通知只落库
 * （LLM 上下文），唤醒轮流式化后输出经 chat_stream 推送。快照（打开/重连）
 * 与断点对齐（fetch）是权威兜底。
 *
 * 可靠性分层：
 * - 消息类永久可靠：落库 + 订阅快照（打开/重连 replace）+ 断点对齐兜底；
 * - 任务实时类尽力而为：通道丢事件可接受，日志文件兜底。
 *
 * 断线重连：EventSource 自动重连；重连后重放订阅（快照 replace），
 * 与数据库收敛到一致（DSH 模式，不使用 Last-Event-ID）。
 */

import { useEffect, useRef } from 'react';
import { getApiBase } from '@/lib/api-base';
import { useAppStore } from '@/lib/store';
import { routeChatStreamEvent } from '@/lib/chat-stream';
import type { ChatMessage } from '@/lib/types';

/** 统一事件（ADR-028 事件契约；ADR-029 新增 snapshot 订阅快照）。 */
export interface UnifiedEvent {
  type: 'chat_stream' | 'snapshot' | 'task_status' | 'command_output';
  /** chat_stream / snapshot 事件：会话 ID（子代理 = task_id）。 */
  session_id?: string;
  /** snapshot 事件（ADR-029）：订阅快照——完整历史 + cursor。 */
  messages?: ChatMessage[];
  /** snapshot 事件：快照时的最新持久化序号。 */
  cursor?: number;
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
 * 首个订阅者建立、最后一个退订时关闭。chat_stream 事件由本 hook 路由到
 * 活跃归约器（lib/chat-stream），组件无需重复处理。
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
/** 已订阅会话集合（ADR-029：resident——打开过保持订阅，切回零延迟）。 */
const subscribedSessions = new Set<string>();
/** EventSource 连接是否已建立（onopen 置位；订阅前必须等待——否则快照帧
 * 推入广播通道时无订阅者被丢弃，历史加载静默失败）。 */
let esReady = false;
const esReadyWaiters: (() => void)[] = [];

/** 等待 EventSource 连接就绪（订阅 POST 前调用，消除快照帧丢失竞态）。
 * 测试环境（jsdom 无 EventSource）直接放行——无真实连接，订阅 POST 由
 * mock 处理。 */
function whenEsReady(): Promise<void> {
  if (typeof EventSource === 'undefined') return Promise.resolve();
  if (esReady) return Promise.resolve();
  return new Promise((resolve) => esReadyWaiters.push(resolve));
}

/** 实际订阅请求（等待连接就绪 + POST；失败从集合移除）。 */
async function doSubscribe(sessionId: string): Promise<void> {
  await whenEsReady();
  try {
    const resp = await fetch(getApiBase() + '/events/subscribe', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ session_id: sessionId }),
    });
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
  } catch {
    subscribedSessions.delete(sessionId);
  }
}

/**
 * 订阅会话（ADR-029：快照恢复的触发点）。
 *
 * 幂等（已订阅跳过）；**等待 EventSource 连接就绪后** POST——服务端把
 * 快照（完整历史 + cursor）推入统一事件通道，前端收到 snapshot 帧后
 * replace 窗口——快照与后续实时事件同一条流，顺序由服务端保证，合并
 * 竞态（fetch 历史 + 广播事件两条路径）消失。失败时从集合移除（下次
 * 切换/重连重试）。
 */
export async function subscribeSession(sessionId: string): Promise<void> {
  if (subscribedSessions.has(sessionId)) return;
  subscribedSessions.add(sessionId);
  await doSubscribe(sessionId);
}

/** 测试辅助：重置订阅集合（模块级状态，测试间隔离）。 */
export function __resetSubscriptions(): void {
  subscribedSessions.clear();
}

/**
 * 处理订阅快照帧（ADR-029）：replace 窗口。
 *
 * 流式进行中不 replace（覆盖流式占位有害；增量由实时事件继续）。导出供
 * 测试驱动（测试环境无 EventSource，快照帧由测试直接调用模拟）。
 */
export function applySnapshot(sessionId: string, messages: ChatMessage[]): void {
  const st = useAppStore.getState();
  if ((st.streamStatus[sessionId] ?? 'idle') !== 'streaming') {
    st.setSessionMessages(sessionId, messages);
  }
}

function startUnifiedEvents(): void {
  if (typeof EventSource === 'undefined') return; // 测试环境（jsdom）无 EventSource
  es = new EventSource(getApiBase() + '/events');
  es.onmessage = (e) => {
    try {
      const ev = JSON.parse(e.data) as UnifiedEvent;
      if (ev.type === 'snapshot' && ev.session_id && Array.isArray(ev.messages)) {
        // ADR-029：订阅快照（完整历史 + cursor）→ replace 窗口
        applySnapshot(ev.session_id, ev.messages as ChatMessage[]);
      }
      // ADR-031：message 广播已移除——消息经流式增量 + 完成事件
      // （user_message_id 确认 / pump 边界）到前端；快照（打开/重连）兜底。
      if (ev.type === 'chat_stream') {
        // 对话流式事件（ADR-028 第 3 步）：路由到该会话的活跃归约器
        routeChatStreamEvent(ev as unknown as import('@/lib/types').ChatStreamEvent);
      }
      for (const ref of unifiedEventsListeners) {
        ref.current?.(ev);
      }
    } catch {
      /* 畸形事件跳过 */
    }
  };
  es.onopen = () => {
    // 连接就绪：唤醒等待中的订阅（快照帧推入广播通道时已有订阅者）
    esReady = true;
    for (const resolve of esReadyWaiters.splice(0)) resolve();
    // 重连成功（含首次连接）：重放订阅——服务端订阅状态随连接丢失，
    // 必须强制 POST（绕过幂等检查）；快照 replace 由 snapshot 帧处理
    // （流式会话自动跳过）。
    for (const sid of [...subscribedSessions]) {
      void doSubscribe(sid);
    }
  };
  es.onerror = () => {
    // EventSource 自动重连；重连成功由 onopen 重放订阅。
  };
}

function stopUnifiedEvents(): void {
  es?.close();
  es = null;
  // 连接关闭：就绪标志重置（下次 startUnifiedEvents 重新建立连接后置位）
  esReady = false;
}

