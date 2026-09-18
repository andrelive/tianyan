/**
 * 会话切片（chatSlice）—— 会话/消息/流式状态机的归属地。
 *
 * 局部性原则：全前端最复杂的状态转移（PENDING 迁移、消息合并、segments
 * 时间线、轮次边界、回退定位）只允许在本切片内被推进；组件与流式归约器
 * 只发起意图（调用动作），不直接拼接消息结构。
 */

import type { StateCreator } from 'zustand';
import type {
  ChatMessage,
  MessageSegment,
  Session,
  SkillCallInfo,
  StreamStatus,
  TokenUsage,
  ToolCallEvent,
  ToolResultEvent,
} from '@/lib/types';

/** 待创建会话的本地消息键：新会话首条消息（user + assistant 占位）在服务端
 * 返回 session_id 前暂存于此，首个流式 chunk 到达时经 setCurrentSession
 * 迁移到正式键。跨会话并行流式互不干扰。 */
export const PENDING_SESSION_KEY = '__pending__';
/** 会话消息分页元数据（ADR-035 §8）：上滚分段加载状态。 */
export interface SessionMessageMeta {
  /** 已加载窗口中最旧消息的 seq（null = 未知/未加载）。 */
  oldestSeq: number | null;
  /** 是否还有更早历史（服务端 has_more）。 */
  hasMore: boolean;
}

/** 会话轮状态（ADR-035 §9，U10）：`auto` 轮可中止（前端显示"停止"）。 */
export interface TurnState {
  state: 'running' | 'idle';
  /** 是否自动轮（唤醒轮/子代理轮；false = 用户轮）。 */
  auto: boolean;
}

/** 待回答的追问（composer takeover 数据）：多问题分步（每个问题一个 tab +
 * 补充信息 tab），每个问题含选项（label + description；空 = 纯文本输入），
 * 对齐 DSH ask_user_question 的 questions 数组形态。 */
export interface PendingClarification {
  /** 问题所属会话（U7 跨会话隔离）：气泡只在对应会话显示；
   * 回答也提交回该会话（否则在别的会话回答会错投，原会话永久卡死）。 */
  sessionId: string;
  questions: { question: string; options: { label: string; description?: string | null }[] }[];
}

export interface ChatSlice {
  // Session
  currentSessionId: string | null;
  setCurrentSession: (id: string | null) => void;
  sessions: Session[];
  setSessions: (sessions: Session[]) => void;
  addSession: (session: Session) => void;
  removeSession: (id: string) => void;
  /** 新建对话绑定的工作目录（工作区归属；首条消息时随 ChatRequest 提交） */
  newSessionWorkspace: string | null;
  setNewSessionWorkspace: (dir: string | null) => void;

  // Messages
  // 会话消息缓存：本地真相源（历史加载只做一次；流式按 session_id 归属写入，
  // 切走会话流继续跑，切回直接显示累积内容）。messages 是当前会话投影。
  sessionMessages: Record<string, ChatMessage[]>;
  /** 分页元数据（按会话；上滚加载的状态）。 */
  sessionMessageMeta: Record<string, SessionMessageMeta>;
  /** 轮状态（按会话；驱动输入框与停止按钮，U10）。 */
  turnState: Record<string, TurnState>;
  /** 上滚：把更早的消息插到窗口头部 + 更新分页元数据（ADR-035 §8）。 */
  prependSessionMessages: (
    sessionId: string,
    older: ChatMessage[],
    meta: SessionMessageMeta,
  ) => void;
  /** 设置分页元数据（快照/历史加载后由调用方按服务端 has_more 更新）。 */
  setSessionMessageMeta: (sessionId: string, meta: SessionMessageMeta) => void;
  /** 设置会话轮状态（turn_state 事件驱动，ADR-035 §9）。 */
  setTurnState: (sessionId: string, turn: TurnState) => void;
  messages: ChatMessage[];
  /** 按会话设置消息（更新指定会话缓存；目标会话为当前会话时同步投影）。
   * 用于流结束/失败后从服务端同步真实消息 ID（回退定位键），或后台流
   * 归属非当前会话时的缓存更新。 */
  setSessionMessages: (sessionId: string, messages: ChatMessage[]) => void;
  /** 应用服务端消息边界（chunk_type=message）：按 role 合并到本地最后一条
   * 同角色消息——id/内容以服务端统一结构为准，本地累积的 tool_calls/
   * segments 保留（流式期间已含完整工具结果）。 */
  applyServerMessage: (sessionId: string | null, msg: ChatMessage) => void;
  /** 用户消息落库确认（ADR-031）：比对 user_message_id 把本地乐观消息
   * 替换为服务端真实 id（user_message_id 字段删除——生命周期结束）。 */
  confirmUserMessageId: (
    sessionId: string | null,
    userMessageId: string,
    messageId: string,
  ) => void;
  addMessage: (message: ChatMessage, sessionId?: string | null) => void;
  updateLastMessage: (delta: string, sessionId?: string | null) => void;
  appendSkillCalls: (calls: SkillCallInfo[], sessionId?: string | null) => void;
  /** 累积工具调用事件到指定会话的 assistant 消息（渲染 tool card） */
  appendToolCalls: (calls: ToolCallEvent[], sessionId?: string | null) => void;
  /** 工具执行结果事件（observation chunk）：按 tool_call_id 关联调用卡片，
   * 填充耗时/成败/结果（实时显示 ✓/✗ + 耗时） */
  applyToolResult: (result: ToolResultEvent, sessionId?: string | null) => void;
  /** 累积思考增量到指定会话的 assistant 消息（thinking 字段，折叠展示） */
  appendThinking: (delta: string, sessionId?: string | null) => void;
  /** 开启新的 assistant 轮次消息（流式轮次边界：新一轮 thinking 到达时调用） */
  startNewAssistantTurn: (sessionId?: string | null) => void;
  /** 标记最后一条 assistant 消息为截断（finish_reason === 'length'） */
  markLastMessageTruncated: (sessionId?: string | null) => void;
  /** 标记最后一条 assistant 消息为流式中断（手动停止：残留部分输出打标） */
  markLastMessageInterrupted: (sessionId?: string | null) => void;
  /** 附加 token 用量到 assistant 消息（完成 chunk 携带；按会话独立取数） */
  attachLastMessageUsage: (usage: TokenUsage, sessionId?: string | null) => void;
  clearMessages: () => void;
  /** 回退乐观更新：删除指定会话从 index 起及之后的本地消息（按会话 id
   * 寻址——回退请求期间用户切换会话不得写错对象）。 */
  deleteMessagesFrom: (sessionId: string, index: number) => void;
  /** 指定会话是否已有本地消息缓存（无 → 调用方应加载历史） */
  hasSessionMessages: (sessionId: string) => boolean;

  // Clarification（追问）
  pendingClarification: PendingClarification | null;
  setPendingClarification: (question: PendingClarification | null) => void;
  removeEmptyAssistantMessage: (sessionId?: string | null) => void;

  // Rollback / redo（按消息 ID 定位：前端索引与服务端消息列表错位，
  // 数字索引会删过头——回退/重做都以被删除消息的 ID 为键）
  // **按会话维度存**：回退是会话内操作，切走会话不得残留横幅
  //（U7 追问串台同款根因：全局单字段无会话维度）。
  rollbackMessageBySession: Record<string, string>;
  setRollbackMessage: (sessionId: string, messageId: string | null) => void;

  /** 会话级「上一轮中断原因」（**纯前端内存态**：不落库、不污染 LLM
   * 上下文，应用重启即清）——LLM 请求失败等轮级错误由 ChatPanel 持久
   * 横幅展示；发送新消息或手动关闭时清除。 */
  turnErrorBySession: Record<string, string>;
  /** 设置/清除会话的轮错误（null = 清除；幂等）。 */
  setTurnError: (sessionId: string, message: string | null) => void;

  // Streaming（按会话归属：会话 A 流式时切到 B 可继续发消息，互不阻塞）
  streamStatus: Record<string, StreamStatus>;
  setStreamStatus: (status: StreamStatus, sessionId?: string | null) => void;
  /** 统一事件连接状态（GET /events）。T1-8：断线期间事件全丢，UI 据此提示，
   *  流状态由 `stream-watchdog` 超时兜底复位。 */
  eventsConnected: boolean;
  setEventsConnected: (connected: boolean) => void;
}

/** 解析消息写入目标键：显式 sessionId（流式回调）> 当前会话 > 新会话占位键。 */
function resolveSessionKey(state: ChatSlice, sessionId?: string | null): string {
  return sessionId ?? state.currentSessionId ?? PENDING_SESSION_KEY;
}

/** 更新指定会话的消息列表：同时维护字典与当前会话投影（组件契约不变）。 */
function updateSessionMessages(
  state: ChatSlice,
  sessionId: string | null | undefined,
  updater: (msgs: ChatMessage[]) => ChatMessage[],
): Partial<ChatSlice> {
  const key = resolveSessionKey(state, sessionId);
  // 字典无缓存时回退到当前投影（测试/直设 messages 场景），避免丢消息
  const base =
    state.sessionMessages[key] ?? (key === resolveSessionKey(state) ? state.messages : []);
  const next = updater(base);
  const isCurrent = key === resolveSessionKey(state);
  return {
    sessionMessages: { ...state.sessionMessages, [key]: next },
    ...(isCurrent ? { messages: next } : {}),
  };
}

/** 定位会话消息列表中最后一条 assistant 消息（无则 -1）。 */
function lastAssistantIndex(msgs: ChatMessage[]): number {
  for (let i = msgs.length - 1; i >= 0; i--) {
    if (msgs[i].role === 'assistant') return i;
  }
  return -1;
}

/** 更新最后一条 assistant 消息（不可变；无则原样返回）。 */
function updateLastAssistant(
  msgs: ChatMessage[],
  updater: (msg: ChatMessage) => ChatMessage,
): ChatMessage[] {
  const idx = lastAssistantIndex(msgs);
  if (idx < 0) return msgs;
  const updated = [...msgs];
  updated[idx] = updater(updated[idx]);
  return updated;
}

/**
 * 按 seq 有序插入（ADR-035 §4：seq 是链上位置，落位按位置而非到达顺序）。
 *
 * 无 seq（本地乐观消息等）或找不到更大位置时追加（旧行为）。
 */
function insertBySeq(msgs: ChatMessage[], msg: ChatMessage): void {
  if (msg.seq == null) {
    msgs.push(msg);
    return;
  }
  let insertAt = msgs.length;
  for (let i = 0; i < msgs.length; i++) {
    const s = msgs[i].seq;
    if (s != null && s > msg.seq) {
      insertAt = i;
      break;
    }
  }
  msgs.splice(insertAt, 0, msg);
}

export const createChatSlice: StateCreator<ChatSlice, [], [], ChatSlice> = (set, get) => ({
  // Session
  currentSessionId: null,
  setCurrentSession: (id) =>
    set((s) => {
      if (id === s.currentSessionId) return {};
      let sessionMessages = s.sessionMessages;
      // 新会话创建（null → id）：PENDING 占位消息迁移到正式键，避免首轮
      // 流式消息断链（切走再切回仍完整）。ADR-028：广播可能已把 user 消息
      // 写入正式键（落库即推送）——迁移时合并而非覆盖：正式键已有消息
      // （广播的 user/assistant）优先，PENDING 占位（无 id 的 assistant）
      // 仅在正式键尚无 assistant 时追加（避免重复气泡）。
      if (s.currentSessionId === null && id && sessionMessages[PENDING_SESSION_KEY]?.length) {
        sessionMessages = { ...sessionMessages };
        const pending = sessionMessages[PENDING_SESSION_KEY];
        const existing = sessionMessages[id] ?? [];
        const hasAssistant = existing.some((m) => m.role === 'assistant');
        const hasUser = existing.some((m) => m.role === 'user');
        // 迁移乐观 user 消息（带 user_message_id——ADR-031 确认事件将替换为
        // 真实 id；正式键已有 user（广播版权威）时不迁移）与无 id 的
        // assistant 占位；有 id 的本地消息不迁移（避免与广播的 msg_xxx 重复）
        sessionMessages[id] = hasAssistant
          ? existing
          : [
              ...existing,
              ...pending.filter(
                (m) =>
                  (m.role === 'assistant' && !m.id) ||
                  (m.role === 'user' && !!m.user_message_id && !hasUser),
              ),
            ];
        delete sessionMessages[PENDING_SESSION_KEY];
      }
      // 迁移流式状态键（PENDING → 正式会话）并删除残留键
      const streamStatus = s.streamStatus[PENDING_SESSION_KEY]
        ? (() => {
            const next = { ...s.streamStatus };
            next[id ?? PENDING_SESSION_KEY] = next[PENDING_SESSION_KEY];
            delete next[PENDING_SESSION_KEY];
            return next;
          })()
        : s.streamStatus;
      const key = id ?? PENDING_SESSION_KEY;
      return {
        currentSessionId: id,
        sessionMessages,
        messages: sessionMessages[key] ?? [],
        streamStatus,
      };
    }),
  sessions: [],
  setSessions: (sessions) => set({ sessions }),
  addSession: (session) => set((s) => ({ sessions: [...s.sessions, session] })),
  newSessionWorkspace: null,
  setNewSessionWorkspace: (dir) => set({ newSessionWorkspace: dir }),
  removeSession: (id) =>
    set((s) => {
      const sessions = s.sessions.filter((x) => x.id !== id);
      const wasCurrent = s.currentSessionId === id;
      const currentSessionId = wasCurrent ? null : s.currentSessionId;
      const sessionMessages = { ...s.sessionMessages };
      delete sessionMessages[id];
      const key = currentSessionId ?? PENDING_SESSION_KEY;
      return {
        sessions,
        currentSessionId,
        sessionMessages,
        // 删除当前会话 → 清投影；删除非当前会话 → 保留投影
        // （字典无缓存时回退 messages，兼容直设场景）
        messages: sessionMessages[key] ?? (wasCurrent ? [] : s.messages),
      };
    }),

  // Messages
  sessionMessages: {},
  messages: [],
  setSessionMessages: (sessionId, messages) =>
    set((s) => {
      const isCurrent = sessionId === resolveSessionKey(s);
      const oldestSeq = messages.find((m) => m.seq != null)?.seq ?? null;
      return {
        sessionMessages: { ...s.sessionMessages, [sessionId]: messages },
        sessionMessageMeta: {
          ...s.sessionMessageMeta,
          [sessionId]: {
            oldestSeq,
            hasMore: s.sessionMessageMeta[sessionId]?.hasMore ?? false,
          },
        },
        ...(isCurrent ? { messages } : {}),
      };
    }),
  sessionMessageMeta: {},
  setSessionMessageMeta: (sessionId, meta) =>
    set((s) => ({ sessionMessageMeta: { ...s.sessionMessageMeta, [sessionId]: meta } })),
  prependSessionMessages: (sessionId, older, meta) =>
    set((s) => {
      const base = s.sessionMessages[sessionId] ?? [];
      // 去重：以 seq 为准（服务端分页按 seq 切片，正常不重叠；防御性去重）
      const existing = new Set(base.map((m) => m.seq).filter((x): x is number => x != null));
      const fresh = older.filter((m) => m.seq == null || !existing.has(m.seq));
      const next = [...fresh, ...base];
      const isCurrent = sessionId === resolveSessionKey(s);
      return {
        sessionMessages: { ...s.sessionMessages, [sessionId]: next },
        sessionMessageMeta: { ...s.sessionMessageMeta, [sessionId]: meta },
        ...(isCurrent ? { messages: next } : {}),
      };
    }),
  turnState: {},
  setTurnState: (sessionId, turn) =>
    set((s) => ({ turnState: { ...s.turnState, [sessionId]: turn } })),
  applyServerMessage: (sessionId, msg) =>
    set((s) => {
      const key = sessionId ?? resolveSessionKey(s);
      const base = s.sessionMessages[key] ?? (key === resolveSessionKey(s) ? s.messages : []);
      const next = [...base];
      // user 消息：ADR-031 乐观渲染后本地已有（user_message_id 定位，确认
      // 事件负责 id 同步）——边界事件（用户消息完整版）到达时跳过，避免
      // 重复插入；非乐观场景（无 user_message_id 的请求）仍插入（保持
      // 用户→assistant 顺序：插到最后一条 assistant 占位之前）。
      // 例外：压缩点（compression_marker）是 user 锚定的独立节点（非用户
      // 输入）——跳过乐观合并分支，走下方"按 id 幂等追加/更新"路径
      // （否则未确认的乐观消息在场时会被误判跳过，压缩点丢失）。
      if (msg.role === 'user' && !msg.compression_marker) {
        if (base.some((m) => m.role === 'user' && m.user_message_id)) return {};
        if (msg.id && base.some((m) => m.id === msg.id)) return {};
        const lastAssistant = lastAssistantIndex(next);
        if (lastAssistant >= 0) {
          next.splice(lastAssistant, 0, msg);
        } else {
          next.push(msg);
        }
        const isCurrent = key === resolveSessionKey(s);
        return {
          sessionMessages: { ...s.sessionMessages, [key]: next },
          ...(isCurrent ? { messages: next } : {}),
        };
      }
      // assistant 消息：替换无 id 的占位（保留流式累积的 content/segments，
      // id 同步为服务端 id——回退定位键正确）。
      // 从末尾反向找：边界事件携带流结束时的**最后一条** assistant
      // （多轮工具循环中中间轮占位无 id，但边界只发最后一条）——正向
      // findIndex 会错误替换中间轮占位（中间轮正文被覆盖丢失 + 最后
      // 一条内容重复）。ES2020 无 findLastIndex，手写反向循环。
      if (msg.role === 'assistant') {
        let placeholder = -1;
        for (let i = next.length - 1; i >= 0; i--) {
          if (next[i].role === 'assistant' && !next[i].id) {
            placeholder = i;
            break;
          }
        }
        if (placeholder >= 0) {
          next[placeholder] = { ...next[placeholder], ...msg, id: msg.id };
          const isCurrent = key === resolveSessionKey(s);
          return {
            sessionMessages: { ...s.sessionMessages, [key]: next },
            ...(isCurrent ? { messages: next } : {}),
          };
        }
      }
      // 独立完整消息（system 通知 / 唤醒轮输出 / 无占位的 assistant）：
      // 按 id 查重——已存在则更新（幂等），否则**追加**。
      // 不再按 role 合并：system 通知与唤醒轮输出是独立新消息，
      // 按 role 定位合并会把多条通知/多条输出折叠成一条（历史可见、
      // 实时流只剩一条的根因）。
      if (msg.id) {
        const existing = next.findIndex((m) => m.id === msg.id);
        if (existing >= 0) {
          next[existing] = {
            ...next[existing],
            ...msg,
            id: msg.id,
            tool_calls: next[existing].tool_calls ?? msg.tool_calls,
            segments: msg.segments ?? next[existing].segments,
          };
          const isCurrent = key === resolveSessionKey(s);
          return {
            sessionMessages: { ...s.sessionMessages, [key]: next },
            ...(isCurrent ? { messages: next } : {}),
          };
        }
      }
      // ADR-035 §4：按 seq 落位（有序插入）——取代"纯到达顺序追加"，修复
      // U10 症状三（乐观渲染/多通道并发下的消息顺序倒置）
      insertBySeq(next, msg);
      const isCurrent = key === resolveSessionKey(s);
      return {
        sessionMessages: { ...s.sessionMessages, [key]: next },
        ...(isCurrent ? { messages: next } : {}),
      };
    }),
  confirmUserMessageId: (sessionId, userMessageId, messageId) =>
    set((s) =>
      updateSessionMessages(s, sessionId, (msgs) => {
        const idx = msgs.findIndex((m) => m.user_message_id === userMessageId);
        if (idx < 0) return msgs;
        const updated = [...msgs];
        const { user_message_id: _umid, ...rest } = updated[idx];
        updated[idx] = { ...rest, id: messageId };
        return updated;
      }),
    ),
  addMessage: (message, sessionId) =>
    set((s) =>
      updateSessionMessages(s, sessionId, (msgs) => [
        ...msgs,
        // id: null = 占位（流式期间无 id，等待服务端广播/边界事件替换）；
        // undefined = 普通消息（赋本地 uuid，如压缩摘要）
        { ...message, id: message.id === undefined ? crypto.randomUUID() : message.id },
      ]),
    ),
  updateLastMessage: (delta, sessionId) =>
    set((s) =>
      updateSessionMessages(s, sessionId, (msgs) => {
        // role 守卫：只写最后一条 assistant（文档语义）。列表末条为
        // user/system（如后台通知落尾/乱序到达）时不得写入——防止增量
        // 文本污染非 assistant 消息（跨会话串扰的放大器）。
        const idx = lastAssistantIndex(msgs);
        if (idx < 0) return msgs;
        const last = msgs[idx];
        const segments = last.segments ?? [];
        // 时间线：文本增量若末段已是 text 直接合并，避免逐 delta 建对象
        // （长回答 O(n^2) 段膨胀 → O(n)）
        const lastSeg = segments[segments.length - 1];
        const nextSegments =
          lastSeg && lastSeg.type === 'text'
            ? [...segments.slice(0, -1), { type: 'text' as const, text: lastSeg.text + delta }]
            : [...segments, { type: 'text' as const, text: delta }];
        const updated = [...msgs];
        updated[idx] = {
          ...last,
          segments: nextSegments,
        };
        return updated;
      }),
    ),
  appendSkillCalls: (calls, sessionId) =>
    set((s) =>
      updateSessionMessages(s, sessionId, (msgs) => {
        const idx = lastAssistantIndex(msgs);
        if (idx < 0) return msgs;
        const prev = msgs[idx].skill_calls ?? [];
        // 按 skill_id 去重追加（服务端逐 chunk 发增量；整体覆盖会丢多技能事件）
        const existing = new Set(prev.map((c) => c.skill_id));
        const fresh = calls.filter((c) => !existing.has(c.skill_id));
        if (fresh.length === 0) return msgs;
        const updated = [...msgs];
        updated[idx] = { ...updated[idx], skill_calls: [...prev, ...fresh] };
        return updated;
      }),
    ),
  appendToolCalls: (calls, sessionId) =>
    set((s) =>
      updateSessionMessages(s, sessionId, (msgs) => {
        const idx = lastAssistantIndex(msgs);
        if (idx < 0) return msgs;
        const prev = msgs[idx].tool_calls ?? [];
        // 去重：同一次调用事件只追加一次（chunk 只携带完整事件，无增量合并）。
        // 优先按调用 ID（精确）；无 ID（旧事件/测试）回退 名称+参数。
        const callKey = (c: ToolCallEvent) => c.id ?? `${c.name}${c.arguments}`;
        const existing = new Set(prev.map(callKey));
        const fresh = calls.filter((c) => !existing.has(callKey(c)));
        if (fresh.length === 0) return msgs;
        const updated = [...msgs];
        updated[idx] = {
          ...updated[idx],
          tool_calls: [...prev, ...fresh],
          // 时间线：工具调用按到达顺序追加
          segments: [
            ...(updated[idx].segments ?? []),
            ...fresh.map((call): MessageSegment => ({ type: 'tool', tool_call: call })),
          ],
        };
        return updated;
      }),
    ),
  applyToolResult: (result, sessionId) =>
    set((s) =>
      updateSessionMessages(s, sessionId, (msgs) =>
        msgs.map((m) => {
          if (!m.tool_calls || m.tool_calls.length === 0) return m;
          let changed = false;
          const tool_calls = m.tool_calls.map((c) => {
            if (c.id !== result.tool_call_id) return c;
            changed = true;
            return {
              ...c,
              duration_ms: result.duration_ms,
              success: result.success,
              error: result.error ?? null,
              // 结果内容随 observation 到达：挂到卡片（流式实时显示）
              result: result.content ?? c.result ?? null,
            };
          });
          return changed ? { ...m, tool_calls } : m;
        }),
      ),
    ),
  appendThinking: (delta, sessionId) =>
    set((s) =>
      updateSessionMessages(s, sessionId, (msgs) =>
        updateLastAssistant(msgs, (m) => {
          const segments = m.segments ?? [];
          const lastSeg = segments[segments.length - 1];
          // 时间线：思考增量若末段已是 thinking 直接合并，避免逐 delta
          // 建对象（超长思考流 O(n²) 段膨胀 → O(n)，与正文增量同策略；
          // SegmentBlocks 渲染本就合并相邻 thinking 段，展示不变）
          const nextSegments =
            lastSeg && lastSeg.type === 'thinking'
              ? [
                  ...segments.slice(0, -1),
                  { type: 'thinking' as const, text: lastSeg.text + delta },
                ]
              : [...segments, { type: 'thinking' as const, text: delta }];
          return {
            ...m,
            thinking: (m.thinking ?? '') + delta,
            segments: nextSegments,
          };
        }),
      ),
    ),
  startNewAssistantTurn: (sessionId) =>
    set((s) =>
      updateSessionMessages(s, sessionId, (msgs) => {
        // 轮次边界：新开一条 assistant 消息（与历史"一轮一条消息"语义对齐）。
        // 只在前一条是空占位时复用（流式初始占位），否则追加新消息。
        const last = msgs[msgs.length - 1];
        const isEmptyPlaceholder =
          last?.role === 'assistant' &&
          (!last.segments || last.segments.length === 0) &&
          !last.thinking &&
          (!last.tool_calls || last.tool_calls.length === 0);
        if (isEmptyPlaceholder) return msgs;
        return [
          ...msgs,
          {
            role: 'assistant',
            segments: [],
            timestamp: new Date().toISOString(),
          },
        ];
      }),
    ),
  markLastMessageTruncated: (sessionId) =>
    set((s) =>
      updateSessionMessages(s, sessionId, (msgs) =>
        updateLastAssistant(msgs, (m) => ({ ...m, truncated_by_length: true })),
      ),
    ),
  markLastMessageInterrupted: (sessionId) =>
    set((s) =>
      updateSessionMessages(s, sessionId, (msgs) =>
        updateLastAssistant(msgs, (m) => ({ ...m, interrupted: true })),
      ),
    ),
  attachLastMessageUsage: (usage, sessionId) =>
    set((s) =>
      updateSessionMessages(s, sessionId, (msgs) =>
        updateLastAssistant(msgs, (m) => ({ ...m, usage: { ...m.usage, ...usage } })),
      ),
    ),
  clearMessages: () =>
    set((s) => {
      const key = s.currentSessionId ?? PENDING_SESSION_KEY;
      return {
        sessionMessages: { ...s.sessionMessages, [key]: [] },
        messages: [],
      };
    }),
  deleteMessagesFrom: (sessionId, index) =>
    set((s) => ({
      ...updateSessionMessages(s, sessionId, (msgs) => msgs.slice(0, index)),
      streamStatus: {
        ...s.streamStatus,
        [resolveSessionKey(s, sessionId)]: 'idle',
      },
    })),
  hasSessionMessages: (sessionId) => !!get().sessionMessages[sessionId]?.length,

  // Clarification（追问）
  pendingClarification: null,
  setPendingClarification: (question) => set({ pendingClarification: question }),
  removeEmptyAssistantMessage: (sessionId) =>
    set((s) =>
      updateSessionMessages(s, sessionId, (msgs) => {
        const last = msgs[msgs.length - 1];
        // 只删「真空占位」：无时间线（空白正文/思考/工具）。带 ask_user
        // 工具卡片的助手消息（追问工具链）不是占位——clarification 事件
        // 到达时若误删，追问内容会从信息流消失（只有刷新历史才回来）。
        if (
          last &&
          last.role === 'assistant' &&
          (!last.tool_calls || last.tool_calls.length === 0) &&
          (!last.segments || last.segments.length === 0)
        ) {
          return msgs.slice(0, -1);
        }
        return msgs;
      }),
    ),

  // Rollback / redo
  rollbackMessageBySession: {},
  setRollbackMessage: (sessionId, messageId) =>
    set((s) => {
      const next = { ...s.rollbackMessageBySession };
      if (messageId === null) {
        delete next[sessionId];
      } else {
        next[sessionId] = messageId;
      }
      return { rollbackMessageBySession: next };
    }),
  /** 会话级轮错误（内存态；与 setRollbackMessage 同款会话维度模式）。 */
  turnErrorBySession: {},
  setTurnError: (sessionId, message) =>
    set((s) => {
      const next = { ...s.turnErrorBySession };
      if (message === null) {
        delete next[sessionId];
      } else {
        next[sessionId] = message;
      }
      return { turnErrorBySession: next };
    }),

  // Streaming（按会话归属：切走流继续跑，切回直接显示累积内容）
  eventsConnected: true,
  setEventsConnected: (connected) => set({ eventsConnected: connected }),
  streamStatus: {},
  setStreamStatus: (status, sessionId) =>
    set((s) => {
      const key = resolveSessionKey(s, sessionId);
      return { streamStatus: { ...s.streamStatus, [key]: status } };
    }),
});
