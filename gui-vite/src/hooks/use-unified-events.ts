/**
 * useUnifiedEvents —— 统一事件订阅（ADR-028/031）。
 *
 * 前端启动即建立**应用级常驻** EventSource（GET /events），事件带 type 区分：
 * - `chat_stream`：对话流式事件（ADR-028 第 3 步）→ 纯函数
 *   `handleChatStreamEvent` 直接映射 store（主会话 / 子代理 task_id 同构，
 *   无归约器注册表——唤醒轮事件常驻可达）；
 * - `snapshot`：订阅快照（ADR-029）→ replace 窗口（打开/重连的权威兜底）；
 * - `task_status`：任务状态转移（pending/running/终态）→ 任务面板实时更新；
 * - `command_output`：命令输出增量（尽力而为，终端视图实时 append）。
 *
 * **应用级常驻**：连接生命周期与组件卸载解耦——切到设置/其他面板不关闭
 * EventSource（此前挂在 ChatPanel/AgentTasksPanel 的 useUnifiedEvents 上，
 * 最后一个消费者卸载即 stopUnifiedEvents 关闭连接，订阅集合被丢弃，
 * 切回才由 onopen 重放——实时流在切换期间全部丢失）。模块加载即启动，
 * 进程存活期间不关闭（断线由 EventSource 自动重连 + onopen 重放订阅）。
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
import { handleChatStreamEvent } from '@/lib/chat-stream';
import type { ChatStreamEvent } from '@/lib/types';
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
 * 应用级统一事件订阅（单例 EventSource，按 type 分发）。
 *
 * 组件通过 useUnifiedEvents(handler) 注册回调；连接由模块级启动管理
 * （应用启动即建、进程存活期间常驻），组件卸载只移除回调、不关闭连接。
 * chat_stream 事件经纯函数 `handleChatStreamEvent` 直接映射 store，
 * 组件无需重复处理。
 */
export function useUnifiedEvents(handler?: EventHandler): void {
  const handlerRef = useRef<EventHandler | null>(null);
  handlerRef.current = handler ?? null;

  useEffect(() => {
    const listeners = unifiedEventsListeners;
    listeners.add(handlerRef);
    // 应用级常驻：即使当前无组件订阅 chat_stream，连接也已建立——
    // 归约器为纯函数（无实例），唤醒轮/后台事件不依赖组件存活。
    startUnifiedEventsOnce();
    return () => {
      listeners.delete(handlerRef);
      // 不关闭连接（应用级常驻）；仅移除回调
    };
  }, []);
}

// ── 应用级单例连接管理 ─────────────────────────────────────────

const unifiedEventsListeners = new Set<React.MutableRefObject<EventHandler | null>>();
let started = false;
let es: EventSource | null = null;
/** 当前连接使用的 API base（T1-21：端口变化检测用）。 */
let esBase = '';
/** 已订阅会话集合（ADR-029：resident——打开过保持订阅，切回零延迟）。 */
const subscribedSessions = new Set<string>();
/** EventSource 连接是否已建立（onopen 置位；订阅前必须等待——否则快照帧
 * 推入广播通道时无订阅者被丢弃，历史加载静默失败）。 */
let esReady = false;
const esReadyWaiters: (() => void)[] = [];

/** 首次调用时启动常驻连接（后续调用幂等）。 */
function startUnifiedEventsOnce(): void {
  if (started) return;
  started = true;
  startUnifiedEvents();
}

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

/** 测试辅助：启动统一事件连接（jsdom 无 EventSource，测试自行注入 mock）。 */
export function __startUnifiedEventsForTest(): void {
  startUnifiedEvents();
}

/** 测试辅助：关闭并清空连接与启动标志（模块级状态隔离）。 */
export function __resetUnifiedEventsForTest(): void {
  if (es) {
    es.close();
    es = null;
  }
  esBase = '';
  esReady = false;
  started = false;
}

/**
 * 处理订阅快照帧（ADR-029）：replace 窗口。
 *
 * 流式进行中不 replace（覆盖流式占位有害；增量由实时事件继续）。导出供
 * 测试驱动（测试环境无 EventSource，快照帧由测试直接调用模拟）。
 */
export function applySnapshot(
  sessionId: string,
  messages: ChatMessage[],
  meta?: {
    oldestSeq: number | null;
    hasMore: boolean;
    /** 轮状态权威值（随快照帧下发）：纯通知模型的快照校正来源。 */
    turn?: { state: 'running' | 'idle'; auto: boolean };
  },
): void {
  const st = useAppStore.getState();
  if ((st.streamStatus[sessionId] ?? 'idle') !== 'streaming') {
    st.setSessionMessages(sessionId, messages);
    // ADR-035 §8：快照只含最近 N 条——has_more/游标随快照帧下发，
    // 前端据此启用上滚（loadOlder）。
    if (meta) {
      st.setSessionMessageMeta(sessionId, {
        oldestSeq: meta.oldestSeq,
        hasMore: meta.hasMore,
      });
    }
  }
}

/** 建立应用级常驻连接（模块加载后首次订阅即启动；进程存活期间不关闭，
 * 断线自动重连 + onopen 重放订阅）。 */
function startUnifiedEvents(): void {
  if (typeof EventSource === 'undefined') return; // 测试环境（jsdom）无 EventSource
  if (es) {
    // 重建前关闭旧连接（T1-21 端口变化路径复用本函数）
    es.close();
    es = null;
  }
  // 纯通知模型（看门狗已移除）：轮状态只由事件驱动 + **快照校正**——事件
  // 丢在通道/断线里时，任何一次重连（onopen 重放订阅 → 快照帧携带轮状态
  // 权威值）都会校正，无需超时推断（本地同进程场景没有"后端失联"故障）。
  esBase = getApiBase();
  es = new EventSource(esBase + '/events');
  es.onmessage = (e) => {
    try {
      const ev = JSON.parse(e.data) as UnifiedEvent;
      if (ev.type === 'snapshot' && ev.session_id && Array.isArray(ev.messages)) {
        // ADR-029：订阅快照 → replace 窗口；ADR-035 §8：快照为最近 N 条，
        // 附带 has_more/next_before_seq（上滚游标）
        const raw = ev as unknown as {
          has_more?: boolean;
          next_before_seq?: number | null;
          turn?: { state: 'running' | 'idle'; auto: boolean };
        };
        applySnapshot(ev.session_id, ev.messages as ChatMessage[], {
          oldestSeq: typeof raw.next_before_seq === 'number' ? raw.next_before_seq : null,
          hasMore: Boolean(raw.has_more),
          turn: raw.turn,
        });
      }
      // ADR-031：message 广播已移除——消息经流式增量 + 完成事件
      // （user_message_id 确认 / pump 边界）到前端；快照（打开/重连）兜底。
      // chat_stream 为常驻纯函数处理（无注册表，唤醒轮/后台事件不丢失）。
      if (ev.type === 'chat_stream') {
        handleChatStreamEvent(ev as unknown as ChatStreamEvent);
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
    // T1-8：连接（重）建 → 标记已连接（UI 可提示断线态）
    useAppStore.getState().setEventsConnected(true);
    // 重连成功（含首次连接）：重放订阅——服务端订阅状态随连接丢失，
    // 必须强制 POST（绕过幂等检查）；快照 replace 由 snapshot 帧处理
    // （流式会话自动跳过）。
    for (const sid of [...subscribedSessions]) {
      void doSubscribe(sid);
    }
  };
  es.onerror = () => {
    // EventSource 自动重连（同 URL）；重连成功由 onopen 重放订阅。
    // T1-8：断线期间收不到任何事件（含收尾事件）——标记断线态供 UI 提示；
    // 不在此处立即复位流状态（轮可能仍在服务端跑），由看门狗超时兜底。
    useAppStore.getState().setEventsConnected(false);
    // T1-21：内嵌服务重启后端口可能变化（tauri 重新注入 __TIANYAN_API_BASE__），
    // 而 EventSource 自动重连**永远复用旧 URL** → 实时流永久失联（注入的新
    // 地址只对后续 fetch 生效，事件流不会自己迁移）。base 变化时用新地址重建
    // 连接（订阅由 onopen 重放）；base 未变则交给浏览器自动重连（不打扰）。
    if (getApiBase() !== esBase) {
      esReady = false;
      startUnifiedEvents();
    }
  };
}
