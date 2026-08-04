import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type {
  View,
  StreamStatus,
  Theme,
  FontSize,
  ChatMessage,
  Session,
  Skill,
  SkillCallInfo,
  ToastMessage,
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

  // Messages
  messages: ChatMessage[];
  setMessages: (messages: ChatMessage[]) => void;
  addMessage: (message: ChatMessage) => void;
  updateLastMessage: (delta: string) => void;
  appendSkillCalls: (calls: SkillCallInfo[]) => void;
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
  currentSkillId: string | null;
  setCurrentSkillId: (id: string | null) => void;

  // Toast
  toast: ToastMessage | null;
  showToast: (message: string, type: ToastMessage['type']) => void;
  hideToast: () => void;

  // Model
  selectedModel: string | null;
  setModel: (model: string | null) => void;

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
      currentSkillId: null,
      setCurrentSkillId: (id) => set({ currentSkillId: id }),

      // Toast
      toast: null,
      showToast: (message, type) => set({ toast: { message, type } }),
      hideToast: () => set({ toast: null }),

      // Model
      selectedModel: null,
      setModel: (model) => set({ selectedModel: model }),

      // App mode
      configured: null,
      setConfigured: (val) => set({ configured: val }),
    }),

    {
      name: 'tianyan-ui-preferences',
      partialize: (state) => ({
        theme: state.theme,
        fontSize: state.fontSize,
      }),
    },
  ),
);
