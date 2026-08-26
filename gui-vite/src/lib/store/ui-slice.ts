/**
 * UI 切片（uiSlice）—— 导航/外观/通知/启动状态。
 */

import type { StateCreator } from 'zustand';
import type { FontSize, Theme, ToastMessage, View } from '@/lib/types';

export interface UiSlice {
  // View
  currentView: View;
  setView: (view: View) => void;

  // Settings（本地偏好，persist 持久化 theme/fontSize）
  theme: Theme;
  fontSize: FontSize;
  apiBaseUrl: string;
  setTheme: (theme: Theme) => void;
  setFontSize: (size: FontSize) => void;
  setApiBaseUrl: (url: string) => void;

  // Toast（多实例堆叠：每次 showToast 追加一条，各自 3s 自动消失）
  toasts: (ToastMessage & { id: string })[];
  showToast: (message: string, type: ToastMessage['type']) => void;
  hideToast: (id: string) => void;

  // App mode
  configured: boolean | null;
  setConfigured: (val: boolean) => void;
}

export const createUiSlice: StateCreator<UiSlice, [], [], UiSlice> = (set) => ({
  // View
  currentView: 'chat',
  setView: (view) => set({ currentView: view }),

  // Settings
  theme: 'system',
  fontSize: 'medium',
  apiBaseUrl: 'http://localhost:3000',
  setTheme: (theme) => set({ theme }),
  setFontSize: (size) => set({ fontSize: size }),
  setApiBaseUrl: (url) => set({ apiBaseUrl: url }),

  // Toast
  toasts: [],
  showToast: (message, type) =>
    set((s) => ({
      toasts: [...s.toasts, { id: crypto.randomUUID(), message, type }],
    })),
  hideToast: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),

  // App mode
  configured: null,
  setConfigured: (val) => set({ configured: val }),
});
