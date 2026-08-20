import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type {
  View,
  StreamStatus,
  Theme,
  FontSize,
  ChatMessage,
  ModelInfo,
  MessageSegment,
  Session,
  Skill,
  SkillCallInfo,
  ToastMessage,
  TokenUsage,
  ToolCallEvent,
  ToolResultEvent,
} from './types';

/** 待创建会话的本地消息键：新会话首条消息（user + assistant 占位）在服务端
 * 返回 session_id 前暂存于此，首个流式 chunk 到达时经 setCurrentSession
 * 迁移到正式键。跨会话并行流式互不干扰。 */
export const PENDING_SESSION_KEY = '__pending__';

interface AppState {
  // View
  currentView: View;
  setView: (view: View) => void;

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
  messages: ChatMessage[];
  setMessages: (messages: ChatMessage[]) => void;
  addMessage: (message: ChatMessage, sessionId?: string | null) => void;
  updateLastMessage: (delta: string, sessionId?: string | null) => void;
  appendSkillCalls: (calls: SkillCallInfo[], sessionId?: string | null) => void;
  /** A2：累积工具调用事件到指定会话的 assistant 消息（渲染 tool card） */
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
  /** 附加 token 用量到 assistant 消息（完成 chunk 携带；按会话独立取数） */
  attachLastMessageUsage: (usage: TokenUsage, sessionId?: string | null) => void;
  clearMessages: () => void;
  deleteMessagesFrom: (index: number) => void;
  /** 指定会话是否已有本地消息缓存（无 → 调用方应加载历史） */
  hasSessionMessages: (sessionId: string) => boolean;

  // Clarification（追问）
  pendingClarification: string | null;
  setPendingClarification: (question: string | null) => void;
  removeEmptyAssistantMessage: (sessionId?: string | null) => void;

  // Rollback / redo（按消息 ID 定位：前端索引与服务端消息列表错位，
  // 数字索引会删过头——回退/重做都以被删除消息的 ID 为键）
  lastRollbackMessageId: string | null;
  setLastRollbackMessageId: (id: string | null) => void;

  // Streaming（按会话归属：会话 A 流式时切到 B 可继续发消息，互不阻塞）
  streamStatus: Record<string, StreamStatus>;
  setStreamStatus: (status: StreamStatus, sessionId?: string | null) => void;
  isSidebarOpen: boolean;
  toggleSidebar: () => void;
  setSidebarOpen: (open: boolean) => void;

  // Settings
  theme: Theme;
  fontSize: FontSize;
  apiBaseUrl: string;
  setTheme: (theme: Theme) => void;
  setFontSize: (size: FontSize) => void;
  setApiBaseUrl: (url: string) => void;

  // Skills
  skills: Skill[];
  setSkills: (skills: Skill[]) => void;

  // Toast
  toast: ToastMessage | null;
  showToast: (message: string, type: ToastMessage['type']) => void;
  hideToast: () => void;

  // Model
  selectedModel: string | null;
  setModel: (model: string | null) => void;

  // 会话级思考强度档位（对话时选择，随每次请求下发；值为当前模型声明的档位）
  thinkingEffort: string;
  setThinkingEffort: (effort: string) => void;

  // 聊天模型目录（含每模型思考档位；ThinkingSelect 按当前模型档位渲染）
  chatModels: ModelInfo[];
  setChatModels: (models: ModelInfo[]) => void;

  // App mode
  configured: boolean | null;
  setConfigured: (val: boolean) => void;
}

/** 解析消息写入目标键：显式 sessionId（流式回调）> 当前会话 > 新会话占位键。 */
function resolveSessionKey(state: AppState, sessionId?: string | null): string {
  return sessionId ?? state.currentSessionId ?? PENDING_SESSION_KEY;
}

/** 更新指定会话的消息列表：同时维护字典与当前会话投影（组件契约不变）。 */
function updateSessionMessages(
  state: AppState,
  sessionId: string | null | undefined,
  updater: (msgs: ChatMessage[]) => ChatMessage[],
): Partial<AppState> {
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

export const useAppStore = create<AppState>()(
  persist(
    (set, get) => ({
      // View
      currentView: 'chat',
      setView: (view) => set({ currentView: view }),

      // Session
      currentSessionId: null,
      setCurrentSession: (id) =>
        set((s) => {
          if (id === s.currentSessionId) return {};
          let sessionMessages = s.sessionMessages;
          // 新会话创建（null → id）：PENDING 占位消息迁移到正式键，避免首轮
          // 流式消息断链（切走再切回仍完整）。
          if (s.currentSessionId === null && id && sessionMessages[PENDING_SESSION_KEY]?.length) {
            sessionMessages = { ...sessionMessages };
            sessionMessages[id] = sessionMessages[PENDING_SESSION_KEY];
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
      setMessages: (messages) =>
        set((s) => {
          const key = resolveSessionKey(s);
          return {
            sessionMessages: { ...s.sessionMessages, [key]: messages },
            messages,
          };
        }),
      addMessage: (message, sessionId) =>
        set((s) =>
          updateSessionMessages(s, sessionId, (msgs) => [
            ...msgs,
            { ...message, id: message.id || crypto.randomUUID() },
          ]),
        ),
      updateLastMessage: (delta, sessionId) =>
        set((s) =>
          updateSessionMessages(s, sessionId, (msgs) => {
            if (msgs.length === 0) return msgs;
            const last = msgs[msgs.length - 1];
            const updated = [...msgs];
            updated[updated.length - 1] = {
              ...last,
              content: last.content + delta,
              // 时间线：文本增量按到达顺序追加
              segments: [...(last.segments ?? []), { type: 'text', text: delta }],
            };
            return updated;
          }),
        ),
      appendSkillCalls: (calls, sessionId) =>
        set((s) =>
          updateSessionMessages(s, sessionId, (msgs) => {
            const idx = lastAssistantIndex(msgs);
            if (idx < 0) return msgs;
            const updated = [...msgs];
            updated[idx] = { ...updated[idx], skill_calls: calls };
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
          updateSessionMessages(s, sessionId, (msgs) => {
            const idx = lastAssistantIndex(msgs);
            if (idx < 0) return msgs;
            const updated = [...msgs];
            const prev = updated[idx].thinking ?? '';
            updated[idx] = {
              ...updated[idx],
              thinking: prev + delta,
              // 时间线：思考增量按到达顺序追加
              segments: [...(updated[idx].segments ?? []), { type: 'thinking', text: delta }],
            };
            return updated;
          }),
        ),
      startNewAssistantTurn: (sessionId) =>
        set((s) =>
          updateSessionMessages(s, sessionId, (msgs) => {
            // 轮次边界：新开一条 assistant 消息（与历史“一轮一条消息”语义对齐）。
            // 只在前一条是空占位时复用（流式初始占位），否则追加新消息。
            const last = msgs[msgs.length - 1];
            const isEmptyPlaceholder =
              last?.role === 'assistant' &&
              last.content === '' &&
              !last.thinking &&
              (!last.tool_calls || last.tool_calls.length === 0);
            if (isEmptyPlaceholder) return msgs;
            return [
              ...msgs,
              {
                role: 'assistant',
                content: '',
                timestamp: new Date().toISOString(),
              },
            ];
          }),
        ),
      markLastMessageTruncated: (sessionId) =>
        set((s) =>
          updateSessionMessages(s, sessionId, (msgs) => {
            const idx = lastAssistantIndex(msgs);
            if (idx < 0) return msgs;
            const updated = [...msgs];
            updated[idx] = { ...updated[idx], truncated_by_length: true };
            return updated;
          }),
        ),
      attachLastMessageUsage: (usage, sessionId) =>
        set((s) =>
          updateSessionMessages(s, sessionId, (msgs) => {
            const idx = lastAssistantIndex(msgs);
            if (idx < 0) return msgs;
            const updated = [...msgs];
            updated[idx] = {
              ...updated[idx],
              usage: { ...updated[idx].usage, ...usage },
            };
            return updated;
          }),
        ),
      clearMessages: () =>
        set((s) => {
          const key = s.currentSessionId ?? PENDING_SESSION_KEY;
          return {
            sessionMessages: { ...s.sessionMessages, [key]: [] },
            messages: [],
          };
        }),
      deleteMessagesFrom: (index) =>
        set((s) => {
          const key = s.currentSessionId ?? PENDING_SESSION_KEY;
          const next = (s.sessionMessages[key] ?? s.messages).slice(0, index);
          return {
            sessionMessages: { ...s.sessionMessages, [key]: next },
            messages: next,
            streamStatus: { ...s.streamStatus, [key]: 'idle' },
          };
        }),
      hasSessionMessages: (sessionId) => !!get().sessionMessages[sessionId]?.length,

      // Clarification（追问）
      pendingClarification: null,
      setPendingClarification: (question) => set({ pendingClarification: question }),
      removeEmptyAssistantMessage: (sessionId) =>
        set((s) =>
          updateSessionMessages(s, sessionId, (msgs) => {
            const last = msgs[msgs.length - 1];
            if (last && last.role === 'assistant' && last.content === '') {
              return msgs.slice(0, -1);
            }
            return msgs;
          }),
        ),

      // Rollback / redo
      lastRollbackMessageId: null,
      setLastRollbackMessageId: (id) => set({ lastRollbackMessageId: id }),

      // Streaming（按会话：切走流继续跑，切回直接显示累积内容）
      streamStatus: {},
      setStreamStatus: (status, sessionId) =>
        set((s) => {
          const key = resolveSessionKey(s, sessionId);
          return { streamStatus: { ...s.streamStatus, [key]: status } };
        }),

      // Sidebar
      isSidebarOpen: true,
      toggleSidebar: () => set((s) => ({ isSidebarOpen: !s.isSidebarOpen })),
      setSidebarOpen: (open) => set({ isSidebarOpen: open }),

      // Settings
      theme: 'system',
      fontSize: 'medium',
      apiBaseUrl: 'http://localhost:3000',
      setTheme: (theme) => set({ theme }),
      setFontSize: (size) => set({ fontSize: size }),
      setApiBaseUrl: (url) => set({ apiBaseUrl: url }),

      // Skills
      skills: [],
      setSkills: (skills) => set({ skills }),

      // Toast
      toast: null,
      showToast: (message, type) => set({ toast: { message, type } }),
      hideToast: () => set({ toast: null }),

      // Model
      selectedModel: null,
      setModel: (model) => set({ selectedModel: model }),

      // 会话级思考强度（默认关闭；非 off 时对支持思考的模型生效）
      thinkingEffort: 'off',
      setThinkingEffort: (effort) => set({ thinkingEffort: effort }),
      chatModels: [],
      setChatModels: (models) => set({ chatModels: models }),

      // App mode
      configured: null,
      setConfigured: (val) => set({ configured: val }),
    }),

    {
      name: 'tianyan-ui-preferences',
      partialize: (state) => ({
        theme: state.theme,
        fontSize: state.fontSize,
        thinkingEffort: state.thinkingEffort,
      }),
    },
  ),
);
