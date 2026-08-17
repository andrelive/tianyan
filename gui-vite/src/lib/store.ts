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
} from './types';

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
  messages: ChatMessage[];
  setMessages: (messages: ChatMessage[]) => void;
  addMessage: (message: ChatMessage) => void;
  updateLastMessage: (delta: string) => void;
  appendSkillCalls: (calls: SkillCallInfo[]) => void;
  /** A2：累积工具调用事件到当前 assistant 消息（渲染 tool card） */
  appendToolCalls: (calls: ToolCallEvent[]) => void;
  /** 累积思考增量到当前 assistant 消息（thinking 字段，折叠展示） */
  appendThinking: (delta: string) => void;
  /** 开启新的 assistant 轮次消息（流式轮次边界：新一轮 thinking 到达时调用） */
  startNewAssistantTurn: () => void;
  /** 标记最后一条 assistant 消息为截断（finish_reason === 'length'） */
  markLastMessageTruncated: () => void;
  /** 附加 token 用量到当前 assistant 消息（完成 chunk 携带；前端按会话取数） */
  attachLastMessageUsage: (usage: TokenUsage) => void;
  clearMessages: () => void;
  deleteMessagesFrom: (index: number) => void;

  // Clarification（追问）
  pendingClarification: string | null;
  setPendingClarification: (question: string | null) => void;
  removeEmptyAssistantMessage: () => void;

  // Rollback / redo
  lastRollbackIndex: number | null;
  setLastRollbackIndex: (index: number | null) => void;

  // Streaming
  streamStatus: StreamStatus;
  setStreamStatus: (status: StreamStatus) => void;
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

export const useAppStore = create<AppState>()(
  persist(
    (set) => ({
      // View
      currentView: 'chat',
      setView: (view) => set({ currentView: view }),

      // Session
      currentSessionId: null,
      setCurrentSession: (id) => set({ currentSessionId: id }),
      sessions: [],
      setSessions: (sessions) => set({ sessions }),
      addSession: (session) => set((s) => ({ sessions: [...s.sessions, session] })),
      newSessionWorkspace: null,
      setNewSessionWorkspace: (dir) => set({ newSessionWorkspace: dir }),
      removeSession: (id) =>
        set((s) => {
          const sessions = s.sessions.filter((x) => x.id !== id);
          const currentSessionId = s.currentSessionId === id ? null : s.currentSessionId;
          const messages = s.currentSessionId === id ? [] : s.messages;
          return { sessions, currentSessionId, messages };
        }),

      // Messages
      messages: [],
      setMessages: (messages) => set({ messages }),
      addMessage: (message) =>
        set((s) => ({
          messages: [...s.messages, { ...message, id: message.id || crypto.randomUUID() }],
        })),
      updateLastMessage: (delta) =>
        set((s) => {
          const messages = [...s.messages];
          if (messages.length > 0) {
            const last = messages[messages.length - 1];
            messages[messages.length - 1] = {
              ...last,
              content: last.content + delta,
              // 时间线：文本增量按到达顺序追加
              segments: [...(last.segments ?? []), { type: 'text', text: delta }],
            };
          }
          return { messages };
        }),
      appendSkillCalls: (calls) =>
        set((s) => {
          const messages = [...s.messages];
          let lastIdx = messages.length - 1;
          while (lastIdx >= 0 && messages[lastIdx].role !== 'assistant') {
            lastIdx--;
          }
          if (lastIdx >= 0) {
            messages[lastIdx] = { ...messages[lastIdx], skill_calls: calls };
          }
          return { messages };
        }),
      appendToolCalls: (calls) =>
        set((s) => {
          const messages = [...s.messages];
          let lastIdx = messages.length - 1;
          while (lastIdx >= 0 && messages[lastIdx].role !== 'assistant') {
            lastIdx--;
          }
          if (lastIdx >= 0) {
            const prev = messages[lastIdx].tool_calls ?? [];
            // 去重：同一次调用事件只追加一次（chunk 只携带完整事件，无增量合并）
            const existing = new Set(prev.map((c) => c.name + c.arguments));
            const fresh = calls.filter((c) => !existing.has(c.name + c.arguments));
            if (fresh.length > 0) {
              messages[lastIdx] = {
                ...messages[lastIdx],
                tool_calls: [...prev, ...fresh],
                // 时间线：工具调用按到达顺序追加
                segments: [
                  ...(messages[lastIdx].segments ?? []),
                  ...fresh.map(
                    (call): MessageSegment => ({ type: 'tool', tool_call: call }),
                  ),
                ],
              };
            }
          }
          return { messages };
        }),
      appendThinking: (delta) =>
        set((s) => {
          const messages = [...s.messages];
          let lastIdx = messages.length - 1;
          while (lastIdx >= 0 && messages[lastIdx].role !== 'assistant') {
            lastIdx--;
          }
          if (lastIdx >= 0) {
            const prev = messages[lastIdx].thinking ?? '';
            messages[lastIdx] = {
              ...messages[lastIdx],
              thinking: prev + delta,
              // 时间线：思考增量按到达顺序追加
              segments: [...(messages[lastIdx].segments ?? []), { type: 'thinking', text: delta }],
            };
          }
          return { messages };
        }),
      startNewAssistantTurn: () =>
        set((s) => {
          // 轮次边界：新开一条 assistant 消息（与历史“一轮一条消息”语义对齐）。
          // 只在前一条是空占位时复用（流式初始占位），否则追加新消息。
          const last = s.messages[s.messages.length - 1];
          const isEmptyPlaceholder =
            last?.role === 'assistant' &&
            last.content === '' &&
            !last.thinking &&
            (!last.tool_calls || last.tool_calls.length === 0);
          if (isEmptyPlaceholder) return { messages: s.messages };
          return {
            messages: [
              ...s.messages,
              {
                role: 'assistant',
                content: '',
                timestamp: new Date().toISOString(),
              },
            ],
          };
        }),
      markLastMessageTruncated: () =>
        set((s) => {
          const messages = [...s.messages];
          let lastIdx = messages.length - 1;
          while (lastIdx >= 0 && messages[lastIdx].role !== 'assistant') {
            lastIdx--;
          }
          if (lastIdx >= 0) {
            messages[lastIdx] = { ...messages[lastIdx], truncated_by_length: true };
          }
          return { messages };
        }),
      attachLastMessageUsage: (usage) =>
        set((s) => {
          const messages = [...s.messages];
          let lastIdx = messages.length - 1;
          while (lastIdx >= 0 && messages[lastIdx].role !== 'assistant') {
            lastIdx--;
          }
          if (lastIdx >= 0) {
            messages[lastIdx] = {
              ...messages[lastIdx],
              usage: { ...messages[lastIdx].usage, ...usage },
            };
          }
          return { messages };
        }),
      clearMessages: () => set({ messages: [] }),
      deleteMessagesFrom: (index) =>
        set((s) => ({
          messages: s.messages.slice(0, index),
          streamStatus: 'idle',
        })),

      // Clarification（追问）
      pendingClarification: null,
      setPendingClarification: (question) => set({ pendingClarification: question }),
      removeEmptyAssistantMessage: () =>
        set((s) => {
          const messages = [...s.messages];
          const last = messages[messages.length - 1];
          if (last && last.role === 'assistant' && last.content === '') {
            messages.pop();
          }
          return { messages };
        }),

      // Rollback / redo
      lastRollbackIndex: null,
      setLastRollbackIndex: (index) => set({ lastRollbackIndex: index }),

      // Streaming
      streamStatus: 'idle',
      setStreamStatus: (status) => set({ streamStatus: status }),

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
